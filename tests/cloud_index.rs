//! End-to-end test for index creation + value reads over HTTP range requests.
//!
//! Builds a fixture MDF file with 10 channel groups (each carrying a name and
//! comment) × 10 channels (each with a text name; one channel uses a
//! value-to-text conversion). Serves the file via a local `tiny_http` server
//! that honours single-range `Range: bytes=A-B` requests, builds an
//! `MdfIndex` over `CachingRangeReader<HttpRangeReader>`, then reads 5
//! channels using the same reader with caching bypassed.
//!
//! Headline metric: ≤ 10 underlying HTTP requests for the whole flow.

#![cfg(all(feature = "http", not(target_arch = "wasm32")))]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{CachingRangeReader, HttpRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

const GROUPS: usize = 10;
const CHANNELS_PER_GROUP: usize = 10; // 1 master + 9 data
const RECORDS: usize = 200;

fn build_fixture(path: &Path) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let mut prev_cg: Option<String> = None;
    let mut cg_ids: Vec<String> = Vec::new();
    let mut cn_ids_per_group: Vec<Vec<String>> = Vec::new();

    for g in 0..GROUPS {
        let cg_id = writer.add_channel_group(prev_cg.as_deref(), |_| {})?;
        writer.set_channel_group_name(&cg_id, &format!("Group {g}"))?;
        writer.set_channel_group_comment(&cg_id, &format!("Comment for group {g}"))?;

        let group_name = format!("Group {g}");
        let time_name = format!("t_{g}");
        let time_id = writer.add_channel(&cg_id, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.name = Some(time_name.clone());
            ch.bit_count = 64;
        })?;
        writer.set_time_channel(&time_id)?;

        let mut cn_ids = vec![time_id.clone()];
        for j in 1..CHANNELS_PER_GROUP {
            let prev = cn_ids.last().unwrap().clone();
            let chan_name = format!("ch_{g}_{j}");
            let cn_id = writer.add_channel(&cg_id, Some(&prev), |ch| {
                if j % 2 == 0 {
                    ch.data_type = DataType::FloatLE;
                    ch.bit_count = 64;
                } else {
                    ch.data_type = DataType::UnsignedIntegerLE;
                    ch.bit_count = 32;
                }
                ch.name = Some(chan_name.clone());
            })?;
            cn_ids.push(cn_id);
        }

        // One value-to-text conversion on group 0, channel 9.
        if g == 0 {
            let target = cn_ids[CHANNELS_PER_GROUP - 1].clone();
            writer.add_value_to_text_conversion(&[(0, "OK"), (1, "WARN")], "UNK", Some(&target))?;
        }

        cg_ids.push(cg_id.clone());
        cn_ids_per_group.push(cn_ids);
        prev_cg = Some(cg_id);
        let _ = group_name;
    }

    // Write records for each group: master is f64 time, others alternate.
    for (g, cg_id) in cg_ids.iter().enumerate() {
        writer.start_data_block_for_cg(cg_id, 0)?;
        for r in 0..RECORDS {
            let mut record: Vec<DecodedValue> = Vec::with_capacity(CHANNELS_PER_GROUP);
            record.push(DecodedValue::Float(r as f64 * 0.01));
            for j in 1..CHANNELS_PER_GROUP {
                if j % 2 == 0 {
                    record.push(DecodedValue::Float((g * 100 + r) as f64));
                } else if g == 0 && j == 9 {
                    // Value-to-text candidate: alternate 0/1/2 so we exercise the default.
                    record.push(DecodedValue::UnsignedInteger((r % 3) as u64));
                } else {
                    record.push(DecodedValue::UnsignedInteger((r * (j as usize)) as u64));
                }
            }
            writer.write_record(cg_id, &record)?;
        }
        writer.finish_data_block(cg_id)?;
    }

    writer.finalize()?;
    Ok(())
}

struct ServerHandle {
    handle: Option<thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn spawn_range_server(file_path: PathBuf) -> Result<(String, ServerHandle), MdfError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(MdfError::IOError)?;
    let local_addr = listener.local_addr().map_err(MdfError::IOError)?;
    let url = format!("http://{}/file", local_addr);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    let server = tiny_http::Server::from_listener(listener, None).map_err(|e| {
        MdfError::BlockSerializationError(format!("tiny_http listener error: {e}"))
    })?;

    let handle = thread::spawn(move || {
        let bytes = match std::fs::read(&file_path) {
            Ok(b) => b,
            Err(_) => return,
        };
        let total = bytes.len() as u64;

        loop {
            if stop_thread.load(Ordering::SeqCst) {
                break;
            }
            match server.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(req)) => {
                    let range_header = req
                        .headers()
                        .iter()
                        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("range"))
                        .map(|h| h.value.as_str().to_string());

                    if let Some(range) = range_header.as_deref() {
                        if let Some((start, end)) = parse_byte_range(range, total) {
                            let body = &bytes[start as usize..=end as usize];
                            let content_range = format!("bytes {}-{}/{}", start, end, total);
                            let mut resp = tiny_http::Response::from_data(body.to_vec())
                                .with_status_code(206);
                            resp.add_header(
                                tiny_http::Header::from_bytes(
                                    &b"Content-Range"[..],
                                    content_range.as_bytes(),
                                )
                                .unwrap(),
                            );
                            resp.add_header(
                                tiny_http::Header::from_bytes(
                                    &b"Accept-Ranges"[..],
                                    &b"bytes"[..],
                                )
                                .unwrap(),
                            );
                            let _ = req.respond(resp);
                            continue;
                        }
                    }

                    // No (or invalid) Range → return whole file.
                    let mut resp = tiny_http::Response::from_data(bytes.clone());
                    resp.add_header(
                        tiny_http::Header::from_bytes(&b"Accept-Ranges"[..], &b"bytes"[..])
                            .unwrap(),
                    );
                    let _ = req.respond(resp);
                }
                Ok(None) => continue,
                Err(_) => break,
            }
        }
    });

    Ok((
        url,
        ServerHandle {
            handle: Some(handle),
            stop,
        },
    ))
}

fn parse_byte_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let v = value.trim();
    let rest = v.strip_prefix("bytes=")?;
    let (s, e) = rest.split_once('-')?;
    let start: u64 = s.parse().ok()?;
    let end: u64 = if e.is_empty() {
        total - 1
    } else {
        e.parse().ok()?
    };
    if start >= total {
        return None;
    }
    let end = end.min(total - 1);
    Some((start, end))
}

#[test]
fn cloud_index_round_trips_within_budget() -> Result<(), MdfError> {
    let tmp = tempfile::tempdir().map_err(MdfError::IOError)?;
    let mf4 = tmp.path().join("fixture.mf4");
    build_fixture(&mf4)?;
    let file_size = std::fs::metadata(&mf4).map_err(MdfError::IOError)?.len();

    let (url, _server) = spawn_range_server(mf4.clone())?;

    // Give the server a beat to start accepting connections.
    thread::sleep(Duration::from_millis(50));

    let started = Instant::now();

    let http = HttpRangeReader::new(&url)?;
    let mut cached = CachingRangeReader::with_chunk_size(http, 1 << 20);

    let index = MdfIndex::from_range_reader(&mut cached, file_size)?;

    assert_eq!(index.channel_groups.len(), GROUPS, "group count");
    for (i, g) in index.channel_groups.iter().enumerate() {
        assert_eq!(g.channels.len(), CHANNELS_PER_GROUP, "group {i} channels");
        assert_eq!(
            g.name.as_deref(),
            Some(format!("Group {i}").as_str()),
            "group {i} name"
        );
        assert_eq!(
            g.comment.as_deref(),
            Some(format!("Comment for group {i}").as_str()),
            "group {i} comment"
        );
        assert_eq!(g.record_count, RECORDS as u64, "group {i} record count");

        // Master channel name is t_{i}, others are ch_{i}_{j}.
        assert_eq!(g.channels[0].name.as_deref(), Some(format!("t_{i}").as_str()));
        for (j, ch) in g.channels.iter().enumerate().skip(1) {
            assert_eq!(
                ch.name.as_deref(),
                Some(format!("ch_{i}_{j}").as_str()),
                "group {i} channel {j} name"
            );
        }
    }

    let metadata_requests = cached.underlying_requests();

    // Switch to bypass for value reads — large DT bodies should not
    // pollute the chunk cache.
    cached.set_bypass(true);

    let targets: &[(&str, &str)] = &[
        ("Group 0", "ch_0_9"), // value-to-text conversion
        ("Group 2", "t_2"),    // master
        ("Group 4", "ch_4_3"),
        ("Group 7", "ch_7_1"),
        ("Group 9", "ch_9_8"),
    ];

    for (gn, cn) in targets {
        let g = index
            .find_channel_group_by_name(gn)
            .unwrap_or_else(|| panic!("group {gn} not found"));
        let c = index
            .find_channel_by_name(g, cn)
            .unwrap_or_else(|| panic!("channel {cn} not found in {gn}"));
        let vals = index.read_channel_values(g, c, &mut cached)?;
        assert_eq!(vals.len(), RECORDS, "{gn}/{cn} record count");

        if *gn == "Group 0" && *cn == "ch_0_9" {
            // The value-to-text conversion should yield strings.
            let any_string = vals
                .iter()
                .any(|v| matches!(v, Some(DecodedValue::String(_))));
            assert!(any_string, "expected at least one String value via V2T");
        }
    }

    let elapsed = started.elapsed();
    let total_requests = cached.underlying_requests();
    let value_requests = total_requests - metadata_requests;

    eprintln!(
        "metadata_requests={metadata_requests} value_requests={value_requests} \
         total={total_requests} elapsed={:?} cache_hits={}",
        elapsed,
        cached.cache_hits()
    );

    assert!(
        elapsed < Duration::from_secs(2),
        "wall-time {:?} exceeded 2s budget",
        elapsed
    );
    assert!(
        total_requests <= 10,
        "underlying HTTP requests = {total_requests} (>10)"
    );

    Ok(())
}

#[test]
fn caching_range_reader_collapses_small_reads() -> Result<(), MdfError> {
    use mf4_rs::index::{ByteRangeReader, SliceRangeReader};

    // 3 MiB fixture.
    let data: Vec<u8> = (0..(3 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();

    struct Counted {
        inner: SliceRangeReader,
        calls: u64,
    }
    impl ByteRangeReader for Counted {
        type Error = MdfError;
        fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
            self.calls += 1;
            self.inner.read_range(offset, length)
        }
    }

    let inner = Counted {
        inner: SliceRangeReader::new(data.clone()),
        calls: 0,
    };
    let mut cached = CachingRangeReader::with_chunk_size(inner, 1 << 20);

    // 200 small scattered reads inside the first MiB → 1 underlying read.
    for i in 0..200u64 {
        let off = i * 4096;
        let len = 24u64;
        let bytes = cached.read_range(off, len)?;
        assert_eq!(bytes, data[off as usize..(off + len) as usize]);
    }
    assert_eq!(cached.underlying_requests(), 1);

    // Read across into the second chunk → at most 1 more.
    let _ = cached.read_range((1 << 20) - 5, 20)?;
    assert!(cached.underlying_requests() <= 2);

    Ok(())
}
