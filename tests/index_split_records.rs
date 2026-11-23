//! `MdfIndex` reads records that are split across the `##DT` fragments of a
//! `##DL` list — the blocks of a list form one continuous byte stream, so a
//! record may start in one block and end in the next (or a later) one.
//!
//! `Mdf` reads such files (see `read_path_regressions.rs`); every index entry
//! point must give exactly what `Mdf` gives: local (`from_file`,
//! `from_bytes`), JSON round trip, range-reader builds (including the
//! equal-length `##DL` whose fragment sizes are derived, not read), bound
//! readers (`MdfReader` over a slice and over a file) and URLs — for whole
//! reads, record and time windows, `read_many`, `time_range`, VLSD channels
//! and byte ranges.

use std::cell::RefCell;

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{MdfIndex, SliceRangeReader};
use mf4_rs::writer::{ColumnData, MdfWriter};
use mf4_rs::{DecodedValue, Mdf, ReadOptions, Signal, TimeRange};

// ----- file builders ---------------------------------------------------------------

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join("mf4rs_index_split_records");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_str().unwrap().to_string()
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn pad8(b: &mut Vec<u8>) {
    while b.len() % 8 != 0 {
        b.push(0);
    }
}

fn block(id: &[u8; 4], links: &[u64], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(id);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&((24 + links.len() * 8 + data.len()) as u64).to_le_bytes());
    v.extend_from_slice(&(links.len() as u64).to_le_bytes());
    for l in links {
        v.extend_from_slice(&l.to_le_bytes());
    }
    v.extend_from_slice(data);
    v
}

/// Rewrite the first data group's single `##DT` as a `##DL` of fragments
/// with the given data sizes (records may straddle fragments). With
/// `equal_length`, the list is an equal-length one (`dl_flags` bit 0).
/// Returns each fragment's data section as `(file offset, length)`.
fn split_data_block(path: &str, sizes: &[usize], equal_length: Option<u64>) -> Vec<(u64, u64)> {
    let mut b = std::fs::read(path).unwrap();
    let dg = u64_at(&b, 64 + 24) as usize;
    let dt = u64_at(&b, dg + 40) as usize;
    assert_eq!(&b[dt..dt + 4], b"##DT");
    let len = u64_at(&b, dt + 8) as usize - 24;
    assert_eq!(sizes.iter().sum::<usize>(), len, "fragment sizes must cover the data");
    let data = b[dt + 24..dt + 24 + len].to_vec();
    let mut links = vec![0u64];
    let mut sections = Vec::new();
    let mut offsets = Vec::new();
    let mut pos = 0;
    for &size in sizes {
        pad8(&mut b);
        links.push(b.len() as u64);
        sections.push((b.len() as u64 + 24, size as u64));
        offsets.push(pos as u64);
        b.extend(block(b"##DT", &[], &data[pos..pos + size]));
        pos += size;
    }
    pad8(&mut b);
    let dl = b.len() as u64;
    let mut dl_data = vec![u8::from(equal_length.is_some()), 0, 0, 0];
    dl_data.extend_from_slice(&(sizes.len() as u32).to_le_bytes());
    match equal_length {
        Some(l) => dl_data.extend_from_slice(&l.to_le_bytes()),
        None => offsets.iter().for_each(|o| dl_data.extend_from_slice(&o.to_le_bytes())),
    }
    b.extend(block(b"##DL", &links, &dl_data));
    put_u64(&mut b, dg + 40, dl);
    std::fs::write(path, &b).unwrap();
    sections
}

const N: usize = 2000;
/// Time f64 + Val f64 + State u64.
const RECORD: usize = 24;

/// `Time` (linear: 1 + 0.5·i), `Val` (f64, i·7 mod 13) and `State`
/// (value-to-text) over `N` records of 24 bytes.
fn three_channel_file(path: &str) {
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    let g = w.add_channel_group(Some("G")).unwrap();
    let t = w.add_time_channel(&g, "Time").unwrap();
    w.add_linear_conversion(&t, 1.0, 0.5).unwrap();
    w.add_float_channel(&g, "Val").unwrap();
    let s = w.add_int_channel(&g, "State").unwrap();
    w.add_value_to_text_conversion(&s, &[(0, "off"), (1, "on")], "other").unwrap();
    w.start_data_block(&g).unwrap();
    let tv: Vec<f64> = (0..N).map(|i| i as f64).collect();
    let vv: Vec<f64> = (0..N).map(|i| (i * 7 % 13) as f64).collect();
    let sv: Vec<u64> = (0..N).map(|i| (i % 3) as u64).collect();
    w.write_columns(&g, &[ColumnData::F64(&tv), ColumnData::F64(&vv), ColumnData::U64(&sv)]).unwrap();
    w.finish_data_block(&g).unwrap();
    w.finalize().unwrap();
}

/// 48000 bytes over uneven fragments: many records straddle a boundary, one
/// spans three fragments (the 7-byte and 4-byte ones).
const SPLITS: [usize; 6] = [1000, 7, 13_000, 4, 20, 33_969];

fn split_file(name: &str) -> (String, Vec<(u64, u64)>) {
    let path = tmp(name);
    three_channel_file(&path);
    let sections = split_data_block(&path, &SPLITS, None);
    (path, sections)
}

/// VLSD file: `Time` + `Msg` (UTF-8 VLSD) with an invalidation byte;
/// `records[i] = (valid, entry)`.
fn vlsd_file(path: &str, payloads: &[&[u8]], records: &[(bool, usize)]) {
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group_with(None, |_| {}).unwrap();
    let t = w
        .add_channel_with(&cg, None, |c| {
            c.data_type = DataType::FloatLE;
            c.bit_count = 64;
            c.name = Some("Time".into());
        })
        .unwrap();
    w.set_time_channel(&t).unwrap();
    let vlsd = w
        .add_channel_with(&cg, Some(&t), |c| {
            c.data_type = DataType::StringUtf8;
            c.bit_count = 64;
            c.channel_type = 1;
            c.data = 1;
            c.flags = 0x02;
            c.pos_invalidation_bit = 0;
            c.name = Some("Msg".into());
        })
        .unwrap();
    w.start_data_block_for_cg_raw(&cg, 0, 16, 1).unwrap();
    w.start_signal_data_block(&vlsd).unwrap();
    let mut offsets = Vec::new();
    let mut running = 0u64;
    for p in payloads {
        offsets.push(running);
        w.write_signal_data(&vlsd, p).unwrap();
        running += 4 + p.len() as u64;
    }
    for (i, (valid, entry)) in records.iter().enumerate() {
        let mut r = Vec::new();
        r.extend_from_slice(&(i as f64).to_le_bytes());
        r.extend_from_slice(&offsets[*entry].to_le_bytes());
        r.push(if *valid { 0 } else { 1 });
        w.write_raw_record(&cg, &r).unwrap();
    }
    w.finish_signal_data_block(&vlsd).unwrap();
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();
}

// ----- comparing a reader with Mdf --------------------------------------------------

/// The read API of one index entry point.
struct Api<'a> {
    read: Box<dyn Fn(&str, &ReadOptions) -> Result<Signal, MdfError> + 'a>,
    values: Box<dyn Fn(&str, &ReadOptions) -> Result<Vec<f64>, MdfError> + 'a>,
    read_many: Box<dyn Fn(&[&str], &ReadOptions) -> Result<Vec<Signal>, MdfError> + 'a>,
    time_range: Box<dyn Fn(&str) -> Result<Option<TimeRange>, MdfError> + 'a>,
}

fn index_api(index: &MdfIndex) -> Api<'_> {
    Api {
        read: Box::new(|n, o| index.read(n, o)),
        values: Box::new(|n, o| index.values(n, o)),
        read_many: Box::new(|n, o| index.read_many(n, o)),
        time_range: Box::new(|n| index.time_range(n, None)),
    }
}

fn reader_api<'a, R: mf4_rs::ByteRangeReader<Error = MdfError> + 'a>(
    reader: &'a RefCell<mf4_rs::MdfReader<'a, R>>,
) -> Api<'a> {
    Api {
        read: Box::new(move |n, o| reader.borrow_mut().read(n, o)),
        values: Box::new(move |n, o| reader.borrow_mut().values(n, o)),
        read_many: Box::new(move |n, o| reader.borrow_mut().read_many(n, o)),
        time_range: Box::new(move |n| reader.borrow_mut().time_range(n, None)),
    }
}

fn windows(time_axis: &[f64]) -> Vec<ReadOptions> {
    let n = time_axis.len() as u64;
    let t = |i: usize| time_axis[i.min(time_axis.len() - 1)];
    vec![
        ReadOptions::new(),
        ReadOptions::new().raw(true),
        ReadOptions::new().records(0, n),
        ReadOptions::new().records(41, 2), // records 41/42 straddle 1000 and 1007
        ReadOptions::new().records(40, 600),
        ReadOptions::new().records(n - 1, 1),
        ReadOptions::new().records(n / 2, 0),
        ReadOptions::new().time(Some(t(40)), Some(t(43))),
        ReadOptions::new().time(Some(t(41) + 0.1), None).raw(true),
        ReadOptions::new().time(None, Some(t(n as usize / 3))),
    ]
}

fn debug<T: std::fmt::Debug>(v: T) -> String {
    format!("{v:?}") // NaN-safe equality
}

fn assert_matches_mdf(label: &str, mdf: &Mdf, names: &[&str], api: &Api<'_>) {
    let time_axis = mdf.values("Time", None).unwrap();
    for options in windows(&time_axis) {
        for &name in names {
            let want = mdf.read(name, &options).unwrap();
            let got = (api.read)(name, &options)
                .unwrap_or_else(|e| panic!("{label}: read({name}, {options:?}) failed: {e}"));
            assert_eq!(debug(&got.values), debug(&want.values), "{label}: read({name}, {options:?}) values");
            assert_eq!(debug(&got.timestamps), debug(&want.timestamps), "{label}: read({name}, {options:?}) timestamps");
            let got = (api.values)(name, &options)
                .unwrap_or_else(|e| panic!("{label}: values({name}, {options:?}) failed: {e}"));
            assert_eq!(debug(&got), debug(mdf.values(name, &options).unwrap()), "{label}: values({name}, {options:?})");
        }
        let want = mdf.read_many(names, &options).unwrap();
        let got = (api.read_many)(names, &options)
            .unwrap_or_else(|e| panic!("{label}: read_many({options:?}) failed: {e}"));
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(debug(&g.values), debug(&w.values), "{label}: read_many {} {options:?}", w.name);
            assert_eq!(debug(&g.timestamps), debug(&w.timestamps), "{label}: read_many {} {options:?}", w.name);
        }
    }
    for &name in names {
        let got = (api.time_range)(name).unwrap_or_else(|e| panic!("{label}: time_range({name}) failed: {e}"));
        assert_eq!(got, mdf.time_range(name, None).unwrap(), "{label}: time_range({name})");
    }
}

/// Every local and in-memory index entry point against `Mdf`.
fn assert_all_entry_points_match(path: &str, names: &[&str]) -> Result<(), MdfError> {
    let mdf = Mdf::from_file(path)?;
    let bytes = std::fs::read(path).unwrap();
    let size = bytes.len() as u64;

    let from_file = MdfIndex::from_file(path)?;
    assert_matches_mdf("from_file", &mdf, names, &index_api(&from_file));

    let mut from_bytes = MdfIndex::from_bytes(bytes.clone())?;
    from_bytes.set_source(path);
    assert_matches_mdf("from_bytes", &mdf, names, &index_api(&from_bytes));

    let mut from_json = MdfIndex::from_json(&from_file.to_json()?)?;
    from_json.set_source(path);
    assert_matches_mdf("from_json", &mdf, names, &index_api(&from_json));

    let mut remote = MdfIndex::from_range_reader(&mut SliceRangeReader::new(bytes.clone()), size)?;
    remote.set_source(path);
    assert_matches_mdf("from_range_reader", &mdf, names, &index_api(&remote));

    let over_slice = RefCell::new(remote.open(SliceRangeReader::new(bytes)));
    assert_matches_mdf("MdfReader(slice)", &mdf, names, &reader_api(&over_slice));

    let over_file = RefCell::new(from_file.open_file(path)?);
    assert_matches_mdf("MdfReader(file)", &mdf, names, &reader_api(&over_file));
    Ok(())
}

// ----- tests ----------------------------------------------------------------------------

#[test]
fn index_reads_records_split_across_data_blocks() -> Result<(), MdfError> {
    let (path, _) = split_file("split.mf4");
    // Anchor: the index gives the values that were written.
    let index = MdfIndex::from_file(&path)?;
    let want: Vec<f64> = (0..N).map(|i| (i * 7 % 13) as f64).collect();
    assert_eq!(index.values("Val", None)?, want);
    assert_all_entry_points_match(&path, &["Val", "State", "Time"])
}

#[test]
fn index_reads_split_records_of_an_equal_length_data_list() -> Result<(), MdfError> {
    // An equal-length ##DL whose block length (1000) is not a multiple of the
    // 24-byte record: a range-reader build derives the fragment sizes from
    // the list instead of reading the fragment headers.
    let path = tmp("split_equal.mf4");
    three_channel_file(&path);
    let mut sizes = vec![1000; 47];
    sizes.push(N * RECORD - 47 * 1000);
    split_data_block(&path, &sizes, Some(1000));
    assert_all_entry_points_match(&path, &["Val", "State", "Time"])
}

#[test]
fn index_reads_split_vlsd_records() -> Result<(), MdfError> {
    // 3 records of 17 bytes (16 data + 1 invalidation byte) over 20 + 31
    // bytes; record 1 is invalid, records 0 and 2 point at shared / reordered
    // entries.
    let path = tmp("split_vlsd.mf4");
    vlsd_file(&path, &[b"x", b"y"], &[(true, 1), (false, 0), (true, 1)]);
    split_data_block(&path, &[20, 31], None);
    let mdf = Mdf::from_file(&path)?;
    let want = vec![Some(DecodedValue::String("y".into())), None, Some(DecodedValue::String("y".into()))];
    assert_eq!(mdf.read("Msg", None)?.values, want);

    let index = MdfIndex::from_file(&path)?;
    assert_eq!(index.read("Msg", None)?.values, want);
    for (start, count) in [(0, 3), (1, 2), (2, 1), (1, 0)] {
        let options = ReadOptions::new().records(start, count);
        assert_eq!(
            index.read("Msg", &options)?.values,
            want[start as usize..(start + count) as usize].to_vec(),
            "records({start}, {count})"
        );
    }
    let bytes = std::fs::read(&path).unwrap();
    let size = bytes.len() as u64;
    let remote = MdfIndex::from_range_reader(&mut SliceRangeReader::new(bytes.clone()), size)?;
    assert_eq!(remote.open(SliceRangeReader::new(bytes)).read("Msg", None)?.values, want);
    Ok(())
}

/// File offset of stream byte `logical`, given each fragment's data section.
fn file_offset(sections: &[(u64, u64)], logical: u64) -> u64 {
    let mut start = 0;
    for &(offset, len) in sections {
        if logical < start + len {
            return offset + (logical - start);
        }
        start += len;
    }
    panic!("stream byte {logical} is past the end");
}

#[test]
fn index_byte_ranges_cover_split_records() -> Result<(), MdfError> {
    let (path, sections) = split_file("split_ranges.mf4");
    let index = MdfIndex::from_file(&path)?;
    let (field_offset, field_len) = (8u64, 8u64); // `Val` in the 24-byte record

    for (start, count, ranges) in [
        (0, N as u64, index.byte_ranges("Val", None)?),
        (40, 5, index.byte_ranges_for_records("Val", 40, 5, None)?),
        (41, 1, index.byte_ranges_for_records("Val", 41, 1, None)?),
        (583, 3, index.byte_ranges_for_records("Val", 583, 3, None)?),
    ] {
        // Every range lies inside one fragment's data section...
        for &(offset, len) in &ranges {
            assert!(
                sections.iter().any(|&(s, l)| offset >= s && offset + len <= s + l),
                "range ({offset}, {len}) is not inside a data section"
            );
        }
        // ...and every byte of the channel in the selected records is covered.
        for record in start..start + count {
            for byte in 0..field_len {
                let at = file_offset(&sections, record * RECORD as u64 + field_offset + byte);
                assert!(
                    ranges.iter().any(|&(o, l)| at >= o && at < o + l),
                    "byte {byte} of record {record} (file offset {at}) is not covered by {ranges:?}"
                );
            }
        }
        assert!(ranges.len() <= sections.len());
        // Exactly: per fragment, from the first to the last byte of the
        // selected records' fields it holds (computed byte by byte).
        let mut expected: Vec<(u64, u64)> = Vec::new();
        let mut fragment_start = 0u64;
        for &(offset, len) in &sections {
            let in_fragment = (start..start + count)
                .flat_map(|r| (0..field_len).map(move |b| r * RECORD as u64 + field_offset + b))
                .filter(|&logical| logical >= fragment_start && logical < fragment_start + len);
            let (first, last) = in_fragment.fold((u64::MAX, 0), |(lo, hi), l| (lo.min(l), hi.max(l)));
            if first != u64::MAX {
                expected.push((offset + (first - fragment_start), last - first + 1));
            }
            fragment_start += len;
        }
        assert_eq!(ranges, expected, "byte ranges of records {start}..{}", start + count);
    }
    Ok(())
}

#[cfg(feature = "http")]
mod url {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// A thread-per-connection HTTP/1.1 range server for one file.
    struct Server {
        url: String,
        stop: Arc<AtomicBool>,
        requests: Arc<std::sync::atomic::AtomicU64>,
        /// `(first, last)` byte of every range request, in arrival order.
        ranges: Arc<std::sync::Mutex<Vec<(u64, u64)>>>,
    }

    impl Server {
        /// Requests served so far.
        fn requests(&self) -> u64 {
            self.requests.load(Ordering::SeqCst)
        }

        /// The byte ranges requested so far.
        fn ranges(&self) -> Vec<(u64, u64)> {
            self.ranges.lock().unwrap().clone()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn serve(path: &str) -> Server {
        let bytes = Arc::new(std::fs::read(path).unwrap());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/file.mf4", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let requests = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let request_count = requests.clone();
        let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
        let range_log = ranges.clone();
        std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let (bytes, request_count, range_log) = (bytes.clone(), request_count.clone(), range_log.clone());
                std::thread::spawn(move || {
                    stream.set_nonblocking(false).ok();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    // One write per response, sent at once (a separate head and
                    // body write stalls on Nagle + delayed ACK, ~40 ms each).
                    stream.set_nodelay(true).ok();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut stream = stream;
                    loop {
                        let (mut range, mut line, mut saw_request) = (None, String::new(), false);
                        loop {
                            line.clear();
                            match reader.read_line(&mut line) {
                                Ok(0) | Err(_) => return,
                                Ok(_) => {}
                            }
                            if line == "\r\n" || line == "\n" {
                                break;
                            }
                            saw_request = true;
                            if let Some((k, v)) = line.split_once(':')
                                && k.trim().eq_ignore_ascii_case("range")
                            {
                                range = Some(v.trim().to_string());
                            }
                        }
                        if !saw_request {
                            continue;
                        }
                        request_count.fetch_add(1, Ordering::SeqCst);
                        let total = bytes.len() as u64;
                        let range = range.and_then(|r| {
                            let (s, e) = r.strip_prefix("bytes=")?.split_once('-')?;
                            let s: u64 = s.parse().ok()?;
                            let e: u64 = if e.is_empty() { total - 1 } else { e.parse().ok()? };
                            Some((s, e.min(total - 1)))
                        });
                        range_log.lock().unwrap().extend(range);
                        let (head, body): (String, &[u8]) = match range {
                            Some((s, _)) if s >= total => (
                                format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\n\r\n"),
                                &[],
                            ),
                            Some((s, e)) => (
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{e}/{total}\r\nContent-Length: {}\r\n\r\n",
                                    e - s + 1
                                ),
                                &bytes[s as usize..=e as usize],
                            ),
                            None => (format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n"), &bytes[..]),
                        };
                        let mut response = head.into_bytes();
                        response.extend_from_slice(body);
                        if stream.write_all(&response).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Server { url, stop, requests, ranges }
    }

    #[test]
    fn url_index_reads_records_split_across_data_blocks() -> Result<(), MdfError> {
        let (path, _) = split_file("split_url.mf4");
        let mdf = Mdf::from_file(&path)?;
        let server = serve(&path);
        let index = MdfIndex::from_url(&server.url)?;
        assert_matches_mdf("from_url", &mdf, &["Val", "State", "Time"], &index_api(&index));
        Ok(())
    }

    #[test]
    fn url_index_reads_split_records_of_an_equal_length_data_list() -> Result<(), MdfError> {
        let path = tmp("split_equal_url.mf4");
        three_channel_file(&path);
        let mut sizes = vec![1000; 47];
        sizes.push(N * RECORD - 47 * 1000);
        split_data_block(&path, &sizes, Some(1000));
        let mdf = Mdf::from_file(&path)?;
        let server = serve(&path);
        let index = MdfIndex::from_url(&server.url)?;
        assert_matches_mdf("from_url (equal-length)", &mdf, &["Val", "State", "Time"], &index_api(&index));
        Ok(())
    }

    /// A windowed `values()` over a URL must batch its range requests like
    /// `read()` does — not fetch the window one data block at a time.
    #[test]
    fn url_windowed_values_batch_their_requests() -> Result<(), MdfError> {
        let path = tmp("values_batched_url.mf4");
        three_channel_file(&path);
        // 48 blocks of 1000 bytes (records straddle most boundaries).
        split_data_block(&path, &[1000; 48], Some(1000));
        let mdf = Mdf::from_file(&path)?;
        let time_axis = mdf.values("Time", None)?;
        let server = serve(&path);
        let index = MdfIndex::from_url(&server.url)?;
        for options in [
            ReadOptions::new().records(0, N as u64),
            ReadOptions::new().records(100, 1500),
            ReadOptions::new().time(Some(time_axis[100]), Some(time_axis[1900])),
            ReadOptions::new().time(Some(time_axis[100]), Some(time_axis[1900])).raw(true),
        ] {
            let before = server.requests();
            let values = index.values("Val", &options)?;
            let values_requests = server.requests() - before;
            assert_eq!(values, mdf.values("Val", &options)?, "{options:?}");

            let before = server.requests();
            index.read("Val", &options)?;
            let read_requests = server.requests() - before;

            assert!(
                values_requests <= read_requests.max(2) && values_requests <= 8,
                "{options:?}: values() made {values_requests} requests, read() {read_requests}"
            );
        }
        Ok(())
    }

    #[test]
    fn url_windowed_values_prime_deferred_conversions() -> Result<(), MdfError> {
        // A URL-built index defers its conversions. A windowed `values()`
        // must fetch the ones it needs — the channel's (unless raw) and, for
        // a time window, the time axis's — up front in one read-ahead chunk,
        // not block by block in extra request rounds (each starting at the
        // ##CC block itself).
        let path = tmp("values_primed_url.mf4");
        three_channel_file(&path); // Time: linear, State: value-to-text, Val: none
        split_data_block(&path, &[1000; 48], Some(1000));
        let mdf = Mdf::from_file(&path)?;
        let time_axis = mdf.values("Time", None)?;
        let server = serve(&path);
        let index = MdfIndex::from_url(&server.url)?;
        let conversion_blocks: Vec<u64> = index.channel_groups[0]
            .channels
            .iter()
            .map(|c| c.conversion_addr)
            .filter(|&a| a != 0)
            .collect();
        assert_eq!(conversion_blocks.len(), 2, "Time and State carry a conversion");
        let time_window = ReadOptions::new().time(Some(time_axis[100]), Some(time_axis[1900]));
        for (name, options) in [
            ("Val", time_window.clone()),       // the time axis's conversion
            ("Val", time_window.clone().raw(true)),
            ("State", ReadOptions::new().records(100, 1500)), // the channel's
            ("State", time_window.clone()),     // both
        ] {
            let before = server.ranges().len();
            let values = index.values(name, &options)?;
            assert_eq!(debug(&values), debug(mdf.values(name, &options)?), "{name} {options:?}");
            let unprimed: Vec<(u64, u64)> = server.ranges()[before..]
                .iter()
                .copied()
                .filter(|(first, _)| conversion_blocks.contains(first))
                .collect();
            assert!(
                unprimed.is_empty(),
                "{name} {options:?}: conversion blocks fetched on their own: {unprimed:?}"
            );
        }
        Ok(())
    }
}
