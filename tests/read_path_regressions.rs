//! Regression tests for read-path bugs found in a review of the core reader:
//!
//! * records split across `##DT` fragments of a `##DL` list were silently
//!   dropped by `Mdf` (and by `cut` / `merge`), misaligning every later record;
//! * `Mdf` read VLSD values in `##SD` storage order instead of following each
//!   record's offset, and ignored per-record invalidation bits;
//! * `Mdf::values` returned `NaN` for numeric VLSD channels;
//! * integers whose bits span more than 64 bits after the bit offset decoded
//!   wrong, and signed fields wider than 64 bits panicked;
//! * a truncated `##CC` block and a truncated recording panicked instead of
//!   returning an error; a NIL `##DL` link broke local index reads.
//!
//! The windowed-read / conversion tests at the end guard the optimised read
//! paths: they must give exactly what slicing a whole read gives.

use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::common::{BlockParse, DataType};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::error::MdfError;
use mf4_rs::index::MdfIndex;
use mf4_rs::parsing::decoder::{decode_channel_value, decode_f64_from_record};
use mf4_rs::writer::{ColumnData, MdfWriter};
use mf4_rs::{ColumnOptions, DecodedValue, Mdf, ReadOptions, Signal};

// ----- helpers ---------------------------------------------------------------

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join("mf4rs_read_path_regressions");
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

/// A raw block: header, links, data.
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

/// Offset of the first data group's `##DG` block.
fn first_dg(b: &[u8]) -> usize {
    u64_at(b, 64 + 24) as usize
}

/// Rewrite the first data group's single `##DT` block as a `##DL` list of
/// fragments with the given data sizes (records may straddle fragments).
fn split_data_block(path: &str, sizes: &[usize]) {
    let mut b = std::fs::read(path).unwrap();
    let dg = first_dg(&b);
    let dt = u64_at(&b, dg + 40) as usize;
    assert_eq!(&b[dt..dt + 4], b"##DT");
    let len = u64_at(&b, dt + 8) as usize - 24;
    assert_eq!(sizes.iter().sum::<usize>(), len, "fragment sizes must cover the data");
    let data = b[dt + 24..dt + 24 + len].to_vec();
    let mut links = vec![0u64];
    let mut offsets = Vec::new();
    let mut pos = 0;
    for &size in sizes {
        pad8(&mut b);
        links.push(b.len() as u64);
        offsets.push(pos as u64);
        b.extend(block(b"##DT", &[], &data[pos..pos + size]));
        pos += size;
    }
    pad8(&mut b);
    let dl = b.len() as u64;
    let mut dl_data = vec![0u8, 0, 0, 0];
    dl_data.extend_from_slice(&(sizes.len() as u32).to_le_bytes());
    for o in offsets {
        dl_data.extend_from_slice(&o.to_le_bytes());
    }
    b.extend(block(b"##DL", &links, &dl_data));
    put_u64(&mut b, dg + 40, dl);
    std::fs::write(path, &b).unwrap();
}

/// `Time` (f64, i * 0.01) and `Val` (f64, i * 10) for `n` records.
fn time_val_file(path: &str, n: usize) {
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    let g = w.add_channel_group(Some("G")).unwrap();
    w.add_time_channel(&g, "Time").unwrap();
    w.add_float_channel(&g, "Val").unwrap();
    w.start_data_block(&g).unwrap();
    let t: Vec<f64> = (0..n).map(|i| i as f64 * 0.01).collect();
    let v: Vec<f64> = (0..n).map(|i| i as f64 * 10.0).collect();
    w.write_columns(&g, &[ColumnData::F64(&t), ColumnData::F64(&v)]).unwrap();
    w.finish_data_block(&g).unwrap();
    w.finalize().unwrap();
}

fn expected_time(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64 * 0.01).collect()
}

fn expected_val(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64 * 10.0).collect()
}

/// A file whose 10 records (16 bytes each) are split over three fragments of
/// 100, 8 and 52 bytes: record 6 starts in the first and ends in the third.
fn straddled_file(name: &str) -> String {
    let path = tmp(name);
    time_val_file(&path, 10);
    split_data_block(&path, &[100, 8, 52]);
    path
}

/// A VLSD file: `Time` (f64) + `Msg` (VLSD, `data_type`), one invalidation
/// byte per record. `records[i] = (valid, entry)` — record `i` points at the
/// `entry`-th payload of the `##SD` stream.
fn vlsd_file(path: &str, data_type: DataType, payloads: &[&[u8]], records: &[(bool, usize)]) {
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
            c.data_type = data_type.clone();
            c.bit_count = 64;
            c.channel_type = 1;
            c.data = 1;
            c.flags = 0x02; // invalidation bit valid
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

fn text(v: &Option<DecodedValue>) -> Option<String> {
    match v {
        Some(DecodedValue::String(s)) => Some(s.clone()),
        None => None,
        other => panic!("expected a string or None, got {other:?}"),
    }
}

fn texts(s: &Signal) -> Vec<Option<String>> {
    s.values.iter().map(text).collect()
}

fn strs(v: &[Option<&str>]) -> Vec<Option<String>> {
    v.iter().map(|s| s.map(str::to_string)).collect()
}

// ----- records split across data blocks -----------------------------------------

#[test]
fn mdf_reads_records_split_across_data_blocks() -> Result<(), MdfError> {
    let path = straddled_file("straddle_mdf.mf4");
    let mdf = Mdf::from_file(&path)?;
    assert_eq!(mdf.values("Val", None)?, expected_val(10));
    assert_eq!(mdf.values("Time", None)?, expected_time(10));

    let s = mdf.read("Val", None)?;
    assert_eq!(s.values_f64(), expected_val(10));
    assert_eq!(s.timestamps, expected_time(10));

    // Windows over the split records.
    let w = mdf.read("Val", &ReadOptions::new().records(5, 3))?;
    assert_eq!(w.values_f64(), vec![50.0, 60.0, 70.0]);
    assert_eq!(w.timestamps, expected_time(10)[5..8].to_vec());
    let w = mdf.read("Val", &ReadOptions::new().time(Some(0.055), Some(0.085)))?;
    assert_eq!(w.values_f64(), vec![60.0, 70.0, 80.0]);
    assert_eq!(mdf.values("Val", &ReadOptions::new().records(6, 1))?, vec![60.0]);

    let range = mdf.time_range("Val", None)?.unwrap();
    assert_eq!((range.start, range.end), (0.0, expected_time(10)[9]));
    Ok(())
}

#[test]
fn cut_keeps_records_split_across_data_blocks() -> Result<(), MdfError> {
    let path = straddled_file("straddle_cut_in.mf4");
    let out = tmp("straddle_cut_out.mf4");
    mf4_rs::cut_mdf_by_time(&path, &out, 0.0, 1.0)?;
    let mdf = Mdf::from_file(&out)?;
    assert_eq!(mdf.values("Val", None)?, expected_val(10));
    assert_eq!(mdf.values("Time", None)?, expected_time(10));
    Ok(())
}

#[test]
fn merge_keeps_records_split_across_data_blocks() -> Result<(), MdfError> {
    let path = straddled_file("straddle_merge_in.mf4");
    let out = tmp("straddle_merge_out.mf4");
    mf4_rs::merge_files(&out, &path, &path)?;
    let mdf = Mdf::from_file(&out)?;
    let mut want = expected_val(10);
    want.extend(expected_val(10));
    assert_eq!(mdf.values("Val", None)?, want);
    Ok(())
}

#[test]
fn raw_record_iterator_refuses_split_records_instead_of_dropping_them() -> Result<(), MdfError> {
    let path = straddled_file("straddle_raw.mf4");
    let file = mf4_rs::parsing::mdf_file::MdfFile::parse_from_file(&path)?;
    let dg = &file.data_groups[0];
    let cg = &dg.channel_groups[0];
    let records: Vec<Result<&[u8], MdfError>> = cg.raw_channels[1].records(dg, cg, &file.mmap)?.collect();
    let whole: Vec<&[u8]> = records.iter().filter_map(|r| r.as_ref().ok().copied()).collect();
    // Either every record comes back intact, or the split is reported — never
    // a short list of records that silently skips the split one.
    if records.iter().all(Result::is_ok) {
        assert_eq!(whole.len(), 10, "records were silently dropped");
    } else {
        let err = records.iter().find_map(|r| r.as_ref().err()).unwrap().to_string();
        assert!(err.contains("spans"), "unexpected error: {err}");
    }
    Ok(())
}

// ----- VLSD channels ---------------------------------------------------------------

#[test]
fn mdf_vlsd_honours_invalidation_bits() -> Result<(), MdfError> {
    // Record 1 is invalid but still has its own ##SD entry (asammdf style).
    let path = tmp("vlsd_invalid.mf4");
    vlsd_file(&path, DataType::StringUtf8, &[b"a", b"b", b"c"], &[(true, 0), (false, 1), (true, 2)]);
    let mdf = Mdf::from_file(&path)?;
    let index = MdfIndex::from_file(&path)?;
    let want = strs(&[Some("a"), None, Some("c")]);
    assert_eq!(texts(&mdf.read("Msg", None)?), want);
    assert_eq!(texts(&index.read("Msg", None)?), want);
    Ok(())
}

#[test]
fn mdf_vlsd_follows_each_records_offset() -> Result<(), MdfError> {
    // Records 0 and 2 share one entry; record 1 points at an entry stored
    // before the one record 0 uses.
    let path = tmp("vlsd_offsets.mf4");
    vlsd_file(&path, DataType::StringUtf8, &[b"other", b"same"], &[(true, 1), (true, 0), (true, 1)]);
    let mdf = Mdf::from_file(&path)?;
    let index = MdfIndex::from_file(&path)?;
    let want = strs(&[Some("same"), Some("other"), Some("same")]);
    let s = mdf.read("Msg", None)?;
    assert_eq!(texts(&s), want);
    assert_eq!(s.timestamps, vec![0.0, 1.0, 2.0]);
    assert_eq!(texts(&index.read("Msg", None)?), want);

    // Windows select by record, not by ##SD position.
    let w = mdf.read("Msg", &ReadOptions::new().records(1, 2))?;
    assert_eq!(texts(&w), strs(&[Some("other"), Some("same")]));
    let w = mdf.read("Msg", &ReadOptions::new().time(Some(2.0), None))?;
    assert_eq!(texts(&w), strs(&[Some("same")]));
    Ok(())
}

#[test]
fn mdf_vlsd_records_split_across_data_blocks() -> Result<(), MdfError> {
    // 3 records of 17 bytes (16 data + 1 invalidation byte) over 20 + 31 bytes.
    let path = tmp("vlsd_split.mf4");
    vlsd_file(&path, DataType::StringUtf8, &[b"x", b"y", b"z"], &[(true, 2), (false, 1), (true, 0)]);
    split_data_block(&path, &[20, 31]);
    let mdf = Mdf::from_file(&path)?;
    assert_eq!(texts(&mdf.read("Msg", None)?), strs(&[Some("z"), None, Some("x")]));
    Ok(())
}

#[test]
fn raw_records_and_cut_reassemble_split_vlsd_records() -> Result<(), MdfError> {
    // 4 records of 17 bytes over 20 + 7 + 41 bytes: record 1 spans all three
    // fragments. `RawChannel::records` (and `cut`, which uses it) follow the
    // offsets of reassembled records too.
    let path = tmp("vlsd_split_raw.mf4");
    vlsd_file(&path, DataType::StringUtf8, &[b"x", b"y", b"z"], &[(true, 2), (false, 1), (true, 0), (true, 1)]);
    split_data_block(&path, &[20, 7, 41]);

    let file = mf4_rs::parsing::mdf_file::MdfFile::parse_from_file(&path)?;
    let dg = &file.data_groups[0];
    let cg = &dg.channel_groups[0];
    let payloads = cg.raw_channels[1].records(dg, cg, &file.mmap)?.collect::<Result<Vec<_>, _>>()?;
    // The invalid record 1 still points at a real entry, which is returned.
    assert_eq!(payloads, vec![&b"z"[..], &b"y"[..], &b"x"[..], &b"y"[..]]);

    let out = tmp("vlsd_split_raw_cut.mf4");
    mf4_rs::cut_mdf_by_time(&path, &out, 0.0, 10.0)?;
    let cut = Mdf::from_file(&out)?.read("Msg", None)?;
    assert_eq!(texts(&cut), strs(&[Some("z"), None, Some("x"), Some("y")]));
    Ok(())
}

#[test]
fn malformed_vlsd_offset_fields_fail_only_when_read_in_both_readers() -> Result<(), MdfError> {
    // A VLSD channel whose offset field is unusable (0 bits): every record
    // invalid reads as all-`None` in both readers, a valid record errors in
    // both.
    let patch_bit_count = |path: &str, bits: u32| {
        let mut b = std::fs::read(path).unwrap();
        let cg = u64_at(&b, first_dg(&b) + 32) as usize;
        let cn1 = u64_at(&b, cg + 32) as usize;
        let cn2 = u64_at(&b, cn1 + 24) as usize;
        b[cn2 + 96..cn2 + 100].copy_from_slice(&bits.to_le_bytes());
        std::fs::write(path, &b).unwrap();
    };
    let all_invalid = tmp("vlsd_bad_field_invalid.mf4");
    vlsd_file(&all_invalid, DataType::StringUtf8, &[b"a", b"b"], &[(false, 0), (false, 1)]);
    patch_bit_count(&all_invalid, 0);
    assert_eq!(Mdf::from_file(&all_invalid)?.read("Msg", None)?.values, vec![None, None]);
    assert_eq!(MdfIndex::from_file(&all_invalid)?.read("Msg", None)?.values, vec![None, None]);

    let one_valid = tmp("vlsd_bad_field_valid.mf4");
    vlsd_file(&one_valid, DataType::StringUtf8, &[b"a", b"b"], &[(false, 0), (true, 1)]);
    patch_bit_count(&one_valid, 0);
    assert!(Mdf::from_file(&one_valid)?.read("Msg", None).is_err());
    assert!(MdfIndex::from_file(&one_valid)?.read("Msg", None).is_err());
    Ok(())
}

#[test]
fn mdf_values_decodes_numeric_vlsd() -> Result<(), MdfError> {
    let path = tmp("vlsd_numeric.mf4");
    let payloads: Vec<[u8; 8]> = (0..3u64).map(|i| (100 + i).to_le_bytes()).collect();
    let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_slice()).collect();
    vlsd_file(&path, DataType::UnsignedIntegerLE, &refs, &[(true, 0), (true, 1), (true, 2)]);
    let mdf = Mdf::from_file(&path)?;
    let index = MdfIndex::from_file(&path)?;
    assert_eq!(mdf.read("Msg", None)?.values_f64(), vec![100.0, 101.0, 102.0]);
    assert_eq!(mdf.values("Msg", None)?, vec![100.0, 101.0, 102.0]);
    assert_eq!(index.values("Msg", None)?, vec![100.0, 101.0, 102.0]);
    Ok(())
}

#[test]
fn cut_follows_vlsd_record_offsets() -> Result<(), MdfError> {
    let path = tmp("vlsd_cut_in.mf4");
    let out = tmp("vlsd_cut_out.mf4");
    vlsd_file(&path, DataType::StringUtf8, &[b"other", b"same"], &[(true, 1), (true, 0), (true, 1)]);
    mf4_rs::cut_mdf_by_time(&path, &out, 0.0, 10.0)?;
    let mdf = Mdf::from_file(&out)?;
    assert_eq!(texts(&mdf.read("Msg", None)?), strs(&[Some("same"), Some("other"), Some("same")]));
    Ok(())
}

// ----- integer decoding ------------------------------------------------------------

fn int_channel(data_type: DataType, bit_offset: u8, bit_count: u32) -> ChannelBlock {
    let mut cb = ChannelBlock::default();
    cb.data_type = data_type;
    cb.bit_offset = bit_offset;
    cb.bit_count = bit_count;
    cb
}

#[test]
fn integers_spanning_more_than_64_bits_decode_correctly() {
    // A 62-bit field at bit offset 4 occupies 66 bits, i.e. 9 bytes.
    let unsigned: u64 = (1 << 61) | 0x1234_5678_9abc;
    let le = (((unsigned as u128) << 4).to_le_bytes())[..9].to_vec();
    let be = (((unsigned as u128) << 4).to_be_bytes())[7..].to_vec();

    let cb = int_channel(DataType::UnsignedIntegerLE, 4, 62);
    assert_eq!(decode_channel_value(&le, 0, &cb), Some(DecodedValue::UnsignedInteger(unsigned)));
    assert_eq!(decode_f64_from_record(&le, 0, &cb), unsigned as f64);
    let cb = int_channel(DataType::UnsignedIntegerBE, 4, 62);
    assert_eq!(decode_channel_value(&be, 0, &cb), Some(DecodedValue::UnsignedInteger(unsigned)));
    assert_eq!(decode_f64_from_record(&be, 0, &cb), unsigned as f64);

    // -5 as a 62-bit two's complement value.
    let signed: i64 = -5;
    let raw62 = (signed as u64) & ((1u64 << 62) - 1);
    let le = (((raw62 as u128) << 4).to_le_bytes())[..9].to_vec();
    let be = (((raw62 as u128) << 4).to_be_bytes())[7..].to_vec();
    let cb = int_channel(DataType::SignedIntegerLE, 4, 62);
    assert_eq!(decode_channel_value(&le, 0, &cb), Some(DecodedValue::SignedInteger(signed)));
    assert_eq!(decode_f64_from_record(&le, 0, &cb), signed as f64);
    let cb = int_channel(DataType::SignedIntegerBE, 4, 62);
    assert_eq!(decode_channel_value(&be, 0, &cb), Some(DecodedValue::SignedInteger(signed)));
    assert_eq!(decode_f64_from_record(&be, 0, &cb), signed as f64);
}

#[test]
fn integers_wider_than_64_bits_are_rejected_without_panicking() {
    let record = [0xFFu8; 16];
    for data_type in [
        DataType::SignedIntegerLE,
        DataType::SignedIntegerBE,
        DataType::UnsignedIntegerLE,
        DataType::UnsignedIntegerBE,
    ] {
        let cb = int_channel(data_type.clone(), 0, 72);
        let decoded = std::panic::catch_unwind(|| decode_channel_value(&record, 0, &cb));
        assert_eq!(decoded.ok(), Some(None), "{data_type:?}: a 72-bit integer is not decodable");
        let f = std::panic::catch_unwind(|| decode_f64_from_record(&record, 0, &cb));
        assert!(f.is_ok_and(f64::is_nan), "{data_type:?}: a 72-bit integer must decode as NaN");
    }
}

// ----- malformed input must error, not panic ----------------------------------------------

#[test]
fn conversion_block_from_truncated_bytes_errors() {
    // A ##CC header that declares 4 links but the buffer stops after the header.
    let bytes = block(b"##CC", &[], &[]);
    let mut header = bytes.clone();
    put_u64(&mut header, 16, 4);
    for buf in [header.clone(), [header.clone(), vec![0u8; 32]].concat(), [header, vec![0u8; 36]].concat()] {
        let result = std::panic::catch_unwind(|| ConversionBlock::from_bytes(&buf).map(|_| ()));
        assert!(matches!(result, Ok(Err(_))), "a truncated ##CC of {} bytes must be an error", buf.len());
    }
}

#[test]
fn local_index_build_errors_instead_of_panicking_on_truncated_file() {
    let path = tmp("truncated.mf4");
    time_val_file(&path, 1000);
    let b = std::fs::read(&path).unwrap();
    let dt = u64_at(&b, first_dg(&b) + 40) as usize;
    std::fs::write(&path, &b[..dt + 8]).unwrap(); // cut inside the ##DT header
    assert!(Mdf::from_file(&path).is_ok(), "the metadata is intact");
    let built = std::panic::catch_unwind(|| MdfIndex::from_file(&path).map(|_| ()));
    assert!(matches!(built, Ok(Err(_))), "expected an error, got {built:?}");
}

#[test]
fn local_index_skips_nil_data_list_links_like_mdf() -> Result<(), MdfError> {
    let path = tmp("nil_dl_link.mf4");
    time_val_file(&path, 10);
    let mut b = std::fs::read(&path).unwrap();
    let dg = first_dg(&b);
    let dt = u64_at(&b, dg + 40);
    pad8(&mut b);
    let dl = b.len() as u64;
    let mut dl_data = vec![0u8, 0, 0, 0];
    dl_data.extend_from_slice(&2u32.to_le_bytes());
    dl_data.extend_from_slice(&0u64.to_le_bytes());
    dl_data.extend_from_slice(&160u64.to_le_bytes());
    b.extend(block(b"##DL", &[0, dt, 0], &dl_data));
    put_u64(&mut b, dg + 40, dl);
    std::fs::write(&path, &b).unwrap();

    assert_eq!(Mdf::from_file(&path)?.values("Val", None)?, expected_val(10));
    assert_eq!(MdfIndex::from_file(&path)?.values("Val", None)?, expected_val(10));
    Ok(())
}

// ----- the optimised read paths give what slicing a whole read gives --------------------------

fn slice(s: &Signal, start: usize, count: usize) -> (Vec<Option<DecodedValue>>, Vec<f64>) {
    let ts = if s.timestamps.is_empty() { vec![] } else { s.timestamps[start..start + count].to_vec() };
    (s.values[start..start + count].to_vec(), ts)
}

/// Records of a time axis in `[t0, t1]`, by scanning.
fn scan(axis: &[f64], t0: Option<f64>, t1: Option<f64>) -> (usize, usize) {
    let start = axis.iter().position(|&t| t0.is_none_or(|t0| t >= t0)).unwrap_or(axis.len());
    let end = axis.iter().position(|&t| t1.is_some_and(|t1| t > t1)).unwrap_or(axis.len()).max(start);
    (start, end - start)
}

fn check_windows(mdf: &Mdf, name: &str, group: Option<&str>, raw: bool) -> Result<(), MdfError> {
    let base = ReadOptions { group: group.map(str::to_string), raw, ..Default::default() };
    let whole = mdf.read(name, &base)?;
    let n = whole.values.len();
    let axis = if whole.timestamps.is_empty() { mdf.values(name, &ReadOptions { raw: false, ..base.clone() })? } else { whole.timestamps.clone() };
    for (start, count) in [(0, 0), (0, 1), (0, n), (n / 3, 5.min(n - n / 3)), (n - 1, 1), (n, 0)] {
        let opts = ReadOptions { start_record: Some(start as u64), record_count: Some(count as u64), ..base.clone() };
        let got = mdf.read(name, &opts)?;
        let (values, timestamps) = slice(&whole, start, count);
        assert_eq!(got.values, values, "{name} records({start}, {count})");
        assert_eq!(got.timestamps, timestamps, "{name} records({start}, {count}) timestamps");
        let want_f64: Vec<f64> = got.values_f64();
        let got_f64 = mdf.values(name, &opts)?;
        assert_eq!(format!("{got_f64:?}"), format!("{want_f64:?}"), "{name} values records({start}, {count})");
    }
    let (lo, hi) = (axis[0], axis[n - 1]);
    let mid = axis[n / 2];
    for (t0, t1) in [
        (None, None),
        (Some(lo), Some(hi)),
        (Some(mid), None),
        (None, Some(mid)),
        (Some(mid), Some(mid)),
        (Some(hi + 1.0), None),
        (None, Some(lo - 1.0)),
    ] {
        let opts = ReadOptions { start_time: t0, end_time: t1, ..base.clone() };
        let got = mdf.read(name, &opts)?;
        let (start, count) = scan(&axis, t0, t1);
        let (values, timestamps) = slice(&whole, start, count);
        assert_eq!(got.values, values, "{name} time({t0:?}, {t1:?})");
        assert_eq!(got.timestamps, timestamps, "{name} time({t0:?}, {t1:?}) timestamps");
    }
    Ok(())
}

#[test]
fn mdf_windows_match_slices_of_whole_reads() -> Result<(), MdfError> {
    // Linear conversion + value-to-text, several data blocks.
    let path = tmp("windows.mf4");
    let n = 2000;
    let mut w = MdfWriter::new(&path)?;
    w.init_mdf_file()?;
    let g = w.add_channel_group(Some("G"))?;
    let t = w.add_time_channel(&g, "Time")?;
    w.add_linear_conversion(&t, 1.0, 0.5)?;
    let v = w.add_float_channel(&g, "Val")?;
    w.add_linear_conversion(&v, -3.0, 2.0)?;
    let s = w.add_int_channel(&g, "State")?;
    w.add_value_to_text_conversion(&s, &[(0, "off"), (1, "on")], "other")?;
    w.start_data_block(&g)?;
    let tv: Vec<f64> = (0..n).map(|i| i as f64).collect();
    let vv: Vec<f64> = (0..n).map(|i| (i * 7 % 13) as f64).collect();
    let sv: Vec<u64> = (0..n).map(|i| (i % 3) as u64).collect();
    w.write_columns(&g, &[ColumnData::F64(&tv), ColumnData::F64(&vv), ColumnData::U64(&sv)])?;
    w.finish_data_block(&g)?;
    w.finalize()?;
    // 2000 records of 24 bytes over uneven fragments: many split records.
    split_data_block(&path, &[1000, 7, 13_000, 4, 33_989]);

    let mdf = Mdf::from_file(&path)?;
    for name in ["Val", "State", "Time"] {
        check_windows(&mdf, name, None, false)?;
        check_windows(&mdf, name, Some("G"), true)?;
    }
    let range = mdf.time_range("Val", None)?.unwrap();
    assert_eq!((range.start, range.end), (1.0, 1.0 + 0.5 * (n - 1) as f64));
    Ok(())
}

#[test]
fn mdf_windows_match_slices_on_column_oriented_files() -> Result<(), MdfError> {
    let src = tmp("windows_row.mf4");
    let out = tmp("windows_col.mf4");
    time_val_file(&src, 500);
    mf4_rs::to_column_oriented(&src, &out, &ColumnOptions { block_size: 1024, ..Default::default() })?;
    let mdf = Mdf::from_file(&out)?;
    check_windows(&mdf, "Val", Some("G"), false)?;
    check_windows(&mdf, "Time", Some("G"), false)?;
    let many = mdf.read_many(&["Val", "Time"], &ReadOptions::new().time(Some(1.0), Some(2.0)))?;
    let (start, count) = scan(&expected_time(500), Some(1.0), Some(2.0));
    assert_eq!(many[0].values_f64(), expected_val(500)[start..start + count].to_vec());
    assert_eq!(many[0].timestamps, many[1].values_f64());
    Ok(())
}

#[test]
fn mdf_lookups_keep_their_error_behaviour() -> Result<(), MdfError> {
    // A lookup reports the read error of an unreadable name it has to pass
    // (not "channel not found", and not a match found further on).
    let three_channels = |name: &str| -> Result<(String, Vec<u8>), MdfError> {
        let path = tmp(name);
        let mut w = MdfWriter::new(&path)?;
        w.init_mdf_file()?;
        let g = w.add_channel_group(Some("G"))?;
        w.add_time_channel(&g, "Time")?;
        w.add_float_channel(&g, "Val")?;
        w.add_float_channel(&g, "Other")?;
        w.start_data_block(&g)?;
        let v: Vec<f64> = (0..3).map(f64::from).collect();
        w.write_columns(&g, &[ColumnData::F64(&v), ColumnData::F64(&v), ColumnData::F64(&v)])?;
        w.finish_data_block(&g)?;
        w.finalize()?;
        let bytes = std::fs::read(&path).unwrap();
        Ok((path, bytes))
    };
    let expect_read_error = |result: Result<Signal, MdfError>, what: &str| match result {
        Err(MdfError::TooShortBuffer { .. }) => {}
        other => panic!("{what}: expected the name read error, got {other:?}"),
    };
    let in_group = |g: Option<&str>| ReadOptions { group: g.map(str::to_string), ..Default::default() };

    // `Val`'s name link points past the end of the file.
    let (path, mut b) = three_channels("broken_channel_name.mf4")?;
    let cg = u64_at(&b, first_dg(&b) + 32) as usize;
    let cn1 = u64_at(&b, cg + 32) as usize;
    let cn2 = u64_at(&b, cn1 + 24) as usize;
    let past_end = b.len() as u64 + 4096;
    put_u64(&mut b, cn2 + 40, past_end);
    std::fs::write(&path, &b).unwrap();
    let mdf = Mdf::from_file(&path)?;
    for group in [None, Some("G")] {
        // Found before the unreadable name.
        assert_eq!(mdf.values("Time", &in_group(group))?.len(), 3);
        // Must pass the unreadable name: an existing channel after it, and a
        // missing one.
        expect_read_error(mdf.read("Other", &in_group(group)), &format!("Other in {group:?}"));
        expect_read_error(mdf.read("Missing", &in_group(group)), &format!("Missing in {group:?}"));
    }

    // The group's name link points past the end of the file: a lookup by
    // group must read every group's name; one without a group reads none.
    let (path, mut b) = three_channels("broken_group_name.mf4")?;
    let cg = u64_at(&b, first_dg(&b) + 32) as usize;
    let past_end = b.len() as u64 + 4096;
    put_u64(&mut b, cg + 40, past_end);
    std::fs::write(&path, &b).unwrap();
    let mdf = Mdf::from_file(&path)?;
    assert_eq!(mdf.values("Other", None)?.len(), 3);
    expect_read_error(mdf.read("Time", &in_group(Some("G"))), "Time in group G");
    Ok(())
}

#[test]
fn vlsd_offset_fields_narrower_than_64_bits_read_alike() -> Result<(), MdfError> {
    // Out of spec (VLSD offsets are 64-bit), but readable: `Mdf` and
    // `MdfIndex` must agree on a 32-bit offset field followed by another
    // channel's bytes.
    let path = tmp("vlsd_offset32.mf4");
    let mut w = MdfWriter::new(&path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group_with(None, |_| {})?;
    let t = w.add_channel_with(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    let vlsd = w.add_channel_with(&cg, Some(&t), |c| {
        c.data_type = DataType::StringUtf8;
        c.bit_count = 32;
        c.channel_type = 1;
        c.data = 1;
        c.name = Some("Msg".into());
    })?;
    // record: Time f64 | 32-bit offset | 0xFFFFFFFF filler (not part of the offset)
    w.start_data_block_for_cg_raw(&cg, 0, 16, 0)?;
    w.start_signal_data_block(&vlsd)?;
    let payloads: [&[u8]; 3] = [b"first", b"second", b"third"];
    let mut offset = 0u32;
    for (i, p) in payloads.iter().enumerate() {
        let mut r = Vec::new();
        r.extend_from_slice(&(i as f64).to_le_bytes());
        r.extend_from_slice(&offset.to_le_bytes());
        r.extend_from_slice(&[0xFF; 4]);
        w.write_raw_record(&cg, &r)?;
        w.write_signal_data(&vlsd, p)?;
        offset += 4 + p.len() as u32;
    }
    w.finish_signal_data_block(&vlsd)?;
    w.finish_data_block(&cg)?;
    w.finalize()?;
    // The writer always stores 64 for a VLSD channel: patch `Msg`'s
    // cn_bit_count to 32 so the field ends before the 0xFF filler.
    let mut b = std::fs::read(&path).unwrap();
    let cg_addr = u64_at(&b, first_dg(&b) + 32) as usize;
    let cn1 = u64_at(&b, cg_addr + 32) as usize;
    let cn2 = u64_at(&b, cn1 + 24) as usize;
    assert_eq!(u32::from_le_bytes(b[cn2 + 96..cn2 + 100].try_into().unwrap()), 64);
    b[cn2 + 96..cn2 + 100].copy_from_slice(&32u32.to_le_bytes());
    std::fs::write(&path, &b).unwrap();

    let want = strs(&[Some("first"), Some("second"), Some("third")]);
    assert_eq!(texts(&Mdf::from_file(&path)?.read("Msg", None)?), want);
    let index = MdfIndex::from_file(&path)?;
    assert_eq!(texts(&index.read("Msg", None)?), want);
    assert_eq!(texts(&index.read("Msg", &ReadOptions::new().records(1, 2))?), want[1..].to_vec());
    Ok(())
}

// ----- conversions -------------------------------------------------------------------

fn conversion(cc_type: ConversionType, cc_val: Vec<f64>, formula: Option<&str>) -> ConversionBlock {
    ConversionBlock {
        header: Default::default(),
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref: vec![],
        cc_type,
        cc_precision: 0,
        cc_flags: 0,
        cc_ref_count: 0,
        cc_val_count: cc_val.len() as u16,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val,
        formula: formula.map(str::to_string),
        resolved_texts: None,
        resolved_conversions: None,
        default_conversion: None,
    }
}

fn apply(cc: &ConversionBlock, v: DecodedValue) -> DecodedValue {
    cc.apply_decoded(v, &[]).unwrap()
}

#[test]
fn algebraic_conversions_keep_their_results() {
    let cases: &[(&str, f64, f64)] = &[
        ("X * 0.5 + 3", 4.0, 5.0),
        ("X1 * 2", 4.0, 8.0),
        ("sqrt(X) + max(X, 1, 2)", 16.0, 20.0),
        ("2 ^ X - pi * 0", 3.0, 8.0),
        ("X +* 2", 4.0, 4.0), // unparsable: the raw value passes through
        ("Y + 1", 4.0, 4.0),  // unknown variable: passes through
        ("foo(X)", 4.0, 4.0), // unknown function: passes through
    ];
    for _ in 0..3 {
        for &(formula, x, want) in cases {
            let cc = conversion(ConversionType::Algebraic, vec![], Some(formula));
            assert_eq!(apply(&cc, DecodedValue::Float(x)), DecodedValue::Float(want), "{formula} at {x}");
            assert_eq!(apply(&cc, DecodedValue::UnsignedInteger(x as u64)), DecodedValue::Float(want), "{formula}");
        }
    }
    let cc = conversion(ConversionType::Algebraic, vec![], Some("X * 2"));
    assert_eq!(apply(&cc, DecodedValue::String("s".into())), DecodedValue::String("s".into()));
}

/// The table lookup as it was implemented before (linear scan), as reference.
fn reference_lookup(cc_val: &[f64], raw: f64, interp: bool) -> Option<f64> {
    let len = cc_val.len();
    if len < 2 || len % 2 != 0 {
        return None;
    }
    let n = len / 2;
    let table: Vec<(f64, f64)> = (0..n).map(|i| (cc_val[2 * i], cc_val[2 * i + 1])).collect();
    if raw <= table[0].0 {
        return Some(table[0].1);
    }
    if raw >= table[n - 1].0 {
        return Some(table[n - 1].1);
    }
    for i in 0..n - 1 {
        let (k0, v0) = table[i];
        let (k1, v1) = table[i + 1];
        if raw >= k0 && raw <= k1 {
            if interp {
                let t = (raw - k0) / (k1 - k0);
                return Some(v0 + t * (v1 - v0));
            }
            return Some(if k1 - raw < raw - k0 { v1 } else { v0 });
        }
    }
    None
}

#[test]
fn table_lookups_match_the_reference_scan() {
    let tables: Vec<Vec<f64>> = vec![
        vec![5.0, 50.0],
        vec![0.0, 1.0, 10.0, 2.0],
        vec![-4.0, 4.0, -1.0, 1.0, 0.0, 0.0, 2.5, -7.0, 9.0, 3.0],
        // duplicate keys (a step)
        vec![0.0, 0.0, 1.0, 10.0, 1.0, 20.0, 2.0, 30.0],
        (0..256).flat_map(|i| [i as f64 * 3.0, (i * i) as f64]).collect(),
    ];
    let probes: Vec<f64> = (-20..=800).map(|i| i as f64 * 0.37).chain([f64::NAN, -1e300, 1e300, 0.0, 1.0, 2.5]).collect();
    for table in &tables {
        for interp in [true, false] {
            let cc_type = if interp { ConversionType::TableLookupInterp } else { ConversionType::TableLookupNoInterp };
            let cc = conversion(cc_type, table.clone(), None);
            for &x in &probes {
                let want = reference_lookup(table, x, interp).unwrap_or(x);
                match apply(&cc, DecodedValue::Float(x)) {
                    DecodedValue::Float(got) => assert!(
                        got == want || (got.is_nan() && want.is_nan()),
                        "table {table:?} interp {interp} at {x}: {got} != {want}"
                    ),
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }
}
