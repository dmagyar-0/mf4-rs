//! Column-oriented (MDF 4.20) conversion: round-trips through the mmap reader
//! and the index, output layout, streaming under a tiny memory budget, shared
//! time channels, unsorted input, and range-request counts for cloud reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::DataType;
use mf4_rs::column::{to_column_oriented, to_column_oriented_bytes, ColumnOptions, ColumnReport};
use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, CachingRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::signal::Signal;
use mf4_rs::writer::MdfWriter;

const FAST: usize = 20_000;
const SLOW: usize = 1_000;
const UNTIMED: usize = 500;

/// Source fixture with four groups:
/// * `Engine` — Time (f64 master), Speed (f64), Rpm (u16), Gear (u8, value-to-text),
///   Flag (1 bit at bit offset 3 of byte 19)
/// * `Body` — the *same* timestamps as Engine, Temp (f32 with invalidation bit),
///   Name (VLSD string)
/// * `Slow` — its own time axis, Pressure (i32)
/// * `Untimed` — no master, Counter (u32)
fn build_source(path: &Path) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;

    // --- Engine
    let eng = w.add_channel_group(None, |_| {})?;
    w.set_channel_group_name(&eng, "Engine")?;
    let t = w.add_channel_preserving_offsets(&eng, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    let speed = w.add_channel_preserving_offsets(&eng, Some(&t), |c| {
        c.data_type = DataType::FloatLE;
        c.byte_offset = 8;
        c.bit_count = 64;
        c.name = Some("Speed".into());
    })?;
    let rpm = w.add_channel_preserving_offsets(&eng, Some(&speed), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.byte_offset = 16;
        c.bit_count = 16;
        c.name = Some("Rpm".into());
    })?;
    let gear = w.add_channel_preserving_offsets(&eng, Some(&rpm), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.byte_offset = 18;
        c.bit_count = 8;
        c.name = Some("Gear".into());
    })?;
    w.add_value_to_text_conversion(&[(0, "N"), (1, "D"), (2, "R")], "?", Some(&gear))?;
    w.add_channel_preserving_offsets(&eng, Some(&gear), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.byte_offset = 19;
        c.bit_offset = 3;
        c.bit_count = 1;
        c.name = Some("Flag".into());
    })?;

    // --- Body
    let body = w.add_channel_group(None, |_| {})?;
    w.set_channel_group_name(&body, "Body")?;
    let bt = w.add_channel_preserving_offsets(&body, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&bt)?;
    let temp = w.add_channel_preserving_offsets(&body, Some(&bt), |c| {
        c.data_type = DataType::FloatLE;
        c.byte_offset = 8;
        c.bit_count = 32;
        c.flags = 0x02;
        c.pos_invalidation_bit = 0;
        c.name = Some("Temp".into());
    })?;
    let name = w.add_channel_preserving_offsets(&body, Some(&temp), |c| {
        c.data_type = DataType::StringUtf8;
        c.byte_offset = 12;
        c.channel_type = 1;
        c.name = Some("Name".into());
    })?;

    // --- Slow
    let slow = w.add_channel_group(None, |_| {})?;
    w.set_channel_group_name(&slow, "Slow")?;
    let st = w.add_channel_preserving_offsets(&slow, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&st)?;
    w.add_channel_preserving_offsets(&slow, Some(&st), |c| {
        c.data_type = DataType::SignedIntegerLE;
        c.byte_offset = 8;
        c.bit_count = 32;
        c.name = Some("Pressure".into());
    })?;

    // --- Untimed
    let untimed = w.add_channel_group(None, |_| {})?;
    w.set_channel_group_name(&untimed, "Untimed")?;
    w.add_channel_preserving_offsets(&untimed, None, |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.bit_count = 32;
        c.name = Some("Counter".into());
    })?;

    // --- data
    w.start_data_block_for_cg_raw(&eng, 0, 20, 0)?;
    for i in 0..FAST {
        let mut r = Vec::with_capacity(20);
        r.extend_from_slice(&(i as f64 * 0.01).to_le_bytes());
        r.extend_from_slice(&((i as f64).sin() * 100.0).to_le_bytes());
        r.extend_from_slice(&((i * 7 % 65_000) as u16).to_le_bytes());
        r.push((i % 4) as u8);
        // Flag lives in bit 3; set the surrounding bits to junk on purpose.
        r.push(if i % 5 == 0 { 0b1010_1111 } else { 0b0101_0111 });
        w.write_raw_record(&eng, &r)?;
    }
    w.finish_data_block(&eng)?;

    w.start_data_block_for_cg_raw(&body, 0, 20, 1)?;
    w.start_signal_data_block(&name)?;
    let mut sd_offset = 0u64;
    for i in 0..FAST {
        let payload = format!("name-{}", i * 13);
        let mut r = Vec::with_capacity(21);
        r.extend_from_slice(&(i as f64 * 0.01).to_le_bytes());
        r.extend_from_slice(&(20.0f32 + i as f32 * 0.5).to_le_bytes());
        r.extend_from_slice(&sd_offset.to_le_bytes());
        r.push(u8::from(i % 7 == 3)); // invalidation byte
        w.write_raw_record(&body, &r)?;
        w.write_signal_data(&name, payload.as_bytes())?;
        sd_offset += 4 + payload.len() as u64;
    }
    w.finish_data_block(&body)?;
    w.finish_signal_data_block(&name)?;

    w.start_data_block_for_cg_raw(&slow, 0, 12, 0)?;
    for i in 0..SLOW {
        let mut r = Vec::with_capacity(12);
        r.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        r.extend_from_slice(&(i as i32 * -3).to_le_bytes());
        w.write_raw_record(&slow, &r)?;
    }
    w.finish_data_block(&slow)?;

    w.start_data_block_for_cg_raw(&untimed, 0, 4, 0)?;
    for i in 0..UNTIMED {
        w.write_raw_record(&untimed, &(i as u32 * 2).to_le_bytes())?;
    }
    w.finish_data_block(&untimed)?;
    w.finalize()
}

/// `(group, channel)` pairs of every channel in the fixture.
const CHANNELS: &[(&str, &str)] = &[
    ("Engine", "Time"),
    ("Engine", "Speed"),
    ("Engine", "Rpm"),
    ("Engine", "Gear"),
    ("Engine", "Flag"),
    ("Body", "Time"),
    ("Body", "Temp"),
    ("Body", "Name"),
    ("Slow", "Time"),
    ("Slow", "Pressure"),
    ("Untimed", "Counter"),
];

/// When Body's time axis is shared, its time channel *is* Engine's time
/// group, so there is no `Body/Time` in the column file any more.
fn shared_alias(g: &str, c: &str) -> Option<(&'static str, &'static str)> {
    (g == "Body" && c == "Time").then_some(("Engine", "Time"))
}

fn signals_via_mdf(path: &Path) -> Result<BTreeMap<String, Signal>, MdfError> {
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let mut out = BTreeMap::new();
    for (g, c) in CHANNELS {
        let sig = match mdf.signal_in(g, c)? {
            Some(sig) => sig,
            None => {
                let (ag, ac) = shared_alias(g, c).unwrap_or_else(|| panic!("{g}/{c} missing in {}", path.display()));
                mdf.signal_in(ag, ac)?.unwrap()
            }
        };
        out.insert(format!("{g}/{c}"), sig);
    }
    Ok(out)
}

fn signals_via_index(index: &MdfIndex) -> Result<BTreeMap<String, Signal>, MdfError> {
    let mut out = BTreeMap::new();
    for (g, c) in CHANNELS {
        let sig = match index.read_in(g, c) {
            Ok(sig) => sig,
            Err(e) => {
                let (ag, ac) = shared_alias(g, c).ok_or(e)?;
                index.read_in(ag, ac)?
            }
        };
        out.insert(format!("{g}/{c}"), sig);
    }
    Ok(out)
}

fn assert_same(expected: &BTreeMap<String, Signal>, actual: &BTreeMap<String, Signal>, what: &str) {
    for (key, e) in expected {
        let a = &actual[key];
        assert_eq!(a.values.len(), e.values.len(), "{what}: {key} length");
        assert_eq!(a.values, e.values, "{what}: {key} values");
        assert_eq!(a.timestamps, e.timestamps, "{what}: {key} timestamps");
        assert_eq!(a.unit, e.unit, "{what}: {key} unit");
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
    column: PathBuf,
    report: ColumnReport,
}

fn fixture(options: &ColumnOptions) -> Result<Fixture, MdfError> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.mf4");
    let column = dir.path().join("column.mf4");
    build_source(&source)?;
    let report = to_column_oriented(&source, &column, options)?;
    Ok(Fixture { _dir: dir, source, column, report })
}

fn small_blocks() -> ColumnOptions {
    ColumnOptions { block_size: 16 * 1024, ..Default::default() }
}

#[test]
fn column_file_reads_back_identically() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let expected = signals_via_mdf(&f.source)?;

    // Sanity-check the fixture itself.
    let flag = &expected["Engine/Flag"].values;
    assert_eq!(flag[0], Some(DecodedValue::UnsignedInteger(1)));
    assert_eq!(flag[1], Some(DecodedValue::UnsignedInteger(0)));
    assert_eq!(expected["Engine/Gear"].values[2], Some(DecodedValue::String("R".into())));
    assert_eq!(expected["Body/Temp"].values[3], None, "invalid sample");
    assert_eq!(expected["Body/Name"].values[2], Some(DecodedValue::String("name-26".into())));
    assert!(expected["Untimed/Counter"].timestamps.is_empty());

    // Normal (mmap) reader.
    assert_same(&expected, &signals_via_mdf(&f.column)?, "MDF reader");
    // Index built from the local file.
    assert_same(&expected, &signals_via_index(&MdfIndex::from_file(f.column.to_str().unwrap())?)?, "index");
    // Index built with range reads only (the cloud path), read through the file.
    let size = std::fs::metadata(&f.column)?.len();
    let mut reader = mf4_rs::index::FileRangeReader::new(f.column.to_str().unwrap())?;
    let mut index = MdfIndex::from_range_reader(&mut reader, size)?;
    index.set_file(f.column.to_str().unwrap());
    assert_same(&expected, &signals_via_index(&index)?, "range-reader index");
    // ...and after a JSON round-trip of that index.
    let mut reloaded = MdfIndex::from_json(&index.to_json()?)?;
    reloaded.set_file(f.column.to_str().unwrap());
    assert_same(&expected, &signals_via_index(&reloaded)?, "reloaded index");
    Ok(())
}

#[test]
fn column_file_structure_and_sharing() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let r = &f.report;
    assert_eq!(r.source_groups, 4);
    // Engine and Body share one time group; Slow has its own; Untimed none.
    assert_eq!(r.time_groups, 2, "{r:?}");
    assert_eq!(r.shared_time_groups, 1, "{r:?}");
    assert_eq!(r.signal_groups, 8, "{r:?}");
    assert!(r.skipped_channels.is_empty(), "{r:?}");
    assert_eq!(r.file_size, std::fs::metadata(&f.column)?.len());

    let mdf = MDF::from_file(f.column.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 10);
    let mut time_cg_addrs = Vec::new();
    for g in &groups {
        assert_eq!(g.channels().len(), 1, "one channel per group");
        let raw = g.raw_channel_group();
        let ch = &g.channels()[0];
        if ch.block().channel_type == 2 {
            time_cg_addrs.push(raw.addr);
            assert_eq!(raw.block.master_cg_addr, 0);
        } else if g.name()?.as_deref() == Some("Untimed") {
            assert_eq!(raw.block.master_cg_addr, 0);
        } else {
            assert_ne!(raw.block.flags & mf4_rs::column::CG_FLAG_REMOTE_MASTER, 0);
            assert!(time_cg_addrs.contains(&raw.block.master_cg_addr), "master link points at a time group");
        }
        // Single-column records: just the channel's bytes (+1 invalidation byte).
        assert_eq!(ch.block().byte_offset, 0);
    }
    // Body's signals reference Engine's time group.
    let body_temp = groups.iter().find(|g| g.channels()[0].name().ok().flatten().as_deref() == Some("Temp")).unwrap();
    let engine_time = groups.iter().find(|g| g.name().ok().flatten().as_deref() == Some("Engine") && g.channels()[0].block().channel_type == 2).unwrap();
    assert_eq!(body_temp.raw_channel_group().block.master_cg_addr, engine_time.raw_channel_group().addr);
    assert_eq!(body_temp.raw_channel_group().block.invalidation_bytes_nr, 1);

    // Version 4.20.
    let bytes = std::fs::read(&f.column)?;
    assert_eq!(&bytes[8..12], b"4.20");
    assert_eq!(u16::from_le_bytes([bytes[28], bytes[29]]), 420);

    // Layout: every non-data block in the metadata prefix; each signal's data
    // blocks back to back.
    let layout = FileLayout::from_file(f.column.to_str().unwrap())?;
    // Remote-master groups show their 7th link.
    assert_eq!(
        layout.blocks.iter().filter(|b| b.links.iter().any(|l| l.name == "master_cg_addr" && l.target != 0)).count(),
        r.signal_groups - 1, // all but Untimed/Counter
    );
    let data_ids = ["##DV", "##DT", "##SD"];
    for b in &layout.blocks {
        if data_ids.contains(&b.block_type.as_str()) {
            assert!(b.offset >= r.metadata_size, "{} at {} inside metadata", b.block_type, b.offset);
        } else {
            assert!(b.end_offset <= r.metadata_size, "{} at {} after metadata end {}", b.block_type, b.offset, r.metadata_size);
        }
    }
    let index = MdfIndex::from_file(f.column.to_str().unwrap())?;
    let mut multi_block = 0;
    for g in &index.channel_groups {
        for pair in g.data_blocks.windows(2) {
            assert_eq!(pair[1].file_offset, pair[0].file_offset + pair[0].size, "blocks of one signal are contiguous");
        }
        if g.data_blocks.len() > 1 {
            multi_block += 1;
            // Equal-sized blocks (whole records) except the last.
            let first = g.data_blocks[0].size;
            assert!(g.data_blocks[..g.data_blocks.len() - 1].iter().all(|b| b.size == first));
            assert!(first - 24 <= 16 * 1024);
        }
    }
    assert!(multi_block >= 4, "fixture should exercise ##DL splitting");
    Ok(())
}

#[test]
fn streaming_with_tiny_budget_and_chunks_matches() -> Result<(), MdfError> {
    let options = ColumnOptions {
        block_size: 1000, // not a multiple of any record size
        memory_budget: 1,  // forces the 4 KiB minimum per-channel buffer
        read_chunk_size: 1,
        share_identical_time: true,
    };
    let f = fixture(&options)?;
    let expected = signals_via_mdf(&f.source)?;
    assert_same(&expected, &signals_via_mdf(&f.column)?, "tiny budget");
    assert_same(&expected, &signals_via_index(&MdfIndex::from_file(f.column.to_str().unwrap())?)?, "tiny budget index");
    Ok(())
}

#[test]
fn sharing_can_be_disabled() -> Result<(), MdfError> {
    let options = ColumnOptions { share_identical_time: false, ..Default::default() };
    let f = fixture(&options)?;
    assert_eq!(f.report.time_groups, 3);
    assert_eq!(f.report.shared_time_groups, 0);
    assert_same(&signals_via_mdf(&f.source)?, &signals_via_mdf(&f.column)?, "no sharing");
    Ok(())
}

#[test]
fn reconverting_a_column_file_is_stable() -> Result<(), MdfError> {
    // The input already uses remote masters and ##DV/##DL blocks.
    let f = fixture(&small_blocks())?;
    let again = f.column.with_file_name("again.mf4");
    let report = to_column_oriented(&f.column, &again, &ColumnOptions::default())?;
    assert_eq!(report.time_groups, 2);
    assert_eq!(report.signal_groups, 8);
    assert_same(&signals_via_mdf(&f.source)?, &signals_via_mdf(&again)?, "re-converted");
    Ok(())
}

#[test]
fn in_memory_conversion_matches_file_conversion() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let (bytes, report) = to_column_oriented_bytes(&std::fs::read(&f.source)?, &small_blocks())?;
    assert_eq!(report.file_size, bytes.len() as u64);
    let file = std::fs::read(&f.column)?;
    assert_eq!(bytes.len(), file.len());
    // Identical apart from the file-history timestamp.
    let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != file[i]).collect();
    assert!(diff.len() <= 8, "outputs differ at {diff:?}");
    Ok(())
}

#[test]
fn rejects_same_input_and_output() -> Result<(), MdfError> {
    let f = fixture(&ColumnOptions::default())?;
    assert!(to_column_oriented(&f.source, &f.source, &ColumnOptions::default()).is_err());
    Ok(())
}

#[test]
fn wrong_block_sizes_use_the_header_and_wrong_offsets_error() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let path = f.column.to_str().unwrap();
    let index = MdfIndex::from_file(path)?;
    let expected = index.read_in("Engine", "Speed")?;
    let (g, _) = index
        .channel_groups
        .iter()
        .enumerate()
        .find(|(_, g)| g.data_blocks.len() > 1 && g.channels[0].name.as_deref() == Some("Speed"))
        .unwrap();

    // A wrong fragment size (e.g. derived from a stale cycle count): the
    // block header's real length is used, so values are still correct.
    for delta in [8i64, -8] {
        let mut wrong = index.clone();
        let b = &mut wrong.channel_groups[g].data_blocks[1];
        b.size = (b.size as i64 + delta) as u64;
        for mut reloaded in [wrong.clone(), MdfIndex::from_json(&wrong.to_json()?)?] {
            reloaded.set_file(path);
            assert_eq!(reloaded.read_in("Engine", "Speed")?.values, expected.values, "size {delta:+}");
            let mut bound = reloaded.open(mf4_rs::index::FileRangeReader::new(path)?);
            assert_eq!(bound.values_in("Engine", "Speed")?, expected.values, "size {delta:+} (range reader)");
        }
    }

    // A wrong offset lands on something that is not a data block: error.
    let mut wrong = index.clone();
    wrong.channel_groups[g].data_blocks[1].file_offset += 8;
    wrong.set_file(path);
    let err = wrong.read_in("Engine", "Speed").unwrap_err();
    assert!(err.to_string().contains("rebuild the index"), "{err}");
    let mut bound = wrong.open(mf4_rs::index::FileRangeReader::new(path)?);
    assert!(bound.values_in("Engine", "Speed").is_err());
    Ok(())
}

// ---------------------------------------------------------------------------
// Range-request counting
// ---------------------------------------------------------------------------

/// File-backed reader that counts the read calls reaching it.
struct CountingReader {
    inner: mf4_rs::index::FileRangeReader,
    calls: std::rc::Rc<std::cell::Cell<u64>>,
    bytes: std::rc::Rc<std::cell::Cell<u64>>,
}

impl ByteRangeReader for CountingReader {
    type Error = MdfError;
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        self.calls.set(self.calls.get() + 1);
        self.bytes.set(self.bytes.get() + length);
        self.inner.read_range(offset, length)
    }
}

struct CloudCost {
    build_requests: u64,
    build_bytes: u64,
    read_requests: u64,
    read_bytes: u64,
}

/// Requests for "fresh file → index → one signal" against a source served
/// through a 1 MiB read-ahead cache, like `MdfIndex::from_url`.
fn cloud_requests(path: &Path, group: &str, channel: &str) -> Result<(CloudCost, Signal), MdfError> {
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0));
    let reader = CountingReader {
        inner: mf4_rs::index::FileRangeReader::new(path.to_str().unwrap())?,
        calls: calls.clone(),
        bytes: bytes.clone(),
    };
    let mut cached = CachingRangeReader::with_chunk_size(reader, 1 << 20);
    let index = MdfIndex::from_range_reader(&mut cached, std::fs::metadata(path)?.len())?;
    let (build_requests, build_bytes) = (calls.get(), bytes.get());
    let mut bound = index.open(cached);
    bound.reader_mut().set_bypass(true);
    let signal = bound.signal_in(group, channel)?;
    let cost = CloudCost {
        build_requests,
        build_bytes,
        read_requests: calls.get() - build_requests,
        read_bytes: bytes.get() - build_bytes,
    };
    Ok((cost, signal))
}

#[test]
fn single_signal_cloud_read_needs_few_requests() -> Result<(), MdfError> {
    // Large enough that every signal spans several blocks and the data
    // region dwarfs the 1 MiB read-ahead.
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("big.mf4");
    let column = dir.path().join("big_column.mf4");
    let n = 400_000usize;
    let mut w = MdfWriter::new(source.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.set_channel_group_name(&cg, "G")?;
    let mut prev = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&prev)?;
    for k in 0..8 {
        prev = w.add_channel(&cg, Some(&prev), |c| {
            c.data_type = DataType::FloatLE;
            c.bit_count = 64;
            c.name = Some(format!("S{k}"));
        })?;
    }
    w.start_data_block_for_cg(&cg, 0)?;
    let cols: Vec<Vec<f64>> = (0..9).map(|k| (0..n).map(|i| i as f64 * 0.001 + k as f64).collect()).collect();
    let refs: Vec<&[f64]> = cols.iter().map(|c| c.as_slice()).collect();
    w.write_columns_f64(&cg, &refs)?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let report = to_column_oriented(&source, &column, &ColumnOptions { block_size: 512 * 1024, ..Default::default() })?;
    assert!(report.metadata_size < 64 * 1024, "{report:?}");

    let (row, row_sig) = cloud_requests(&source, "G", "S5")?;
    let (col, col_sig) = cloud_requests(&column, "G", "S5")?;
    for (label, c) in [("row-oriented", &row), ("column-oriented", &col)] {
        eprintln!(
            "{label}: index build {} requests / {} KiB, read one signal {} requests / {} KiB",
            c.build_requests,
            c.build_bytes / 1024,
            c.read_requests,
            c.read_bytes / 1024
        );
    }
    assert_eq!(col_sig.values, row_sig.values);
    assert_eq!(col_sig.timestamps, row_sig.timestamps);
    assert_eq!(col_sig.values.len(), n);
    // One request for the metadata prefix; the data region is never touched.
    assert_eq!(col.build_requests, 1, "index build requests");
    // One contiguous range for the signal + one for the shared time channel...
    assert_eq!(col.read_requests, 2, "signal read requests");
    // ...transferring just those two columns (plus block headers).
    let two_columns = 2 * 8 * n as u64;
    assert!(col.read_bytes < two_columns + 4096, "{} bytes", col.read_bytes);
    assert!(row.read_bytes > 4 * col.read_bytes);
    Ok(())
}

// ---------------------------------------------------------------------------
// Unsorted input (hand-built: the writer only produces sorted files)
// ---------------------------------------------------------------------------

fn raw_block(id: &[u8; 4], links: &[u64], data: &[u8]) -> Vec<u8> {
    let len = 24 + links.len() * 8 + data.len();
    let mut b = Vec::new();
    b.extend_from_slice(id);
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&(len as u64).to_le_bytes());
    b.extend_from_slice(&(links.len() as u64).to_le_bytes());
    for l in links {
        b.extend_from_slice(&l.to_le_bytes());
    }
    b.extend_from_slice(data);
    while b.len() % 8 != 0 {
        b.push(0);
    }
    b
}

fn cn_data(channel_type: u8, sync: u8, data_type: u8, byte_offset: u32, bits: u32) -> Vec<u8> {
    let mut d = vec![channel_type, sync, data_type, 0];
    d.extend_from_slice(&byte_offset.to_le_bytes());
    d.extend_from_slice(&bits.to_le_bytes());
    d.extend_from_slice(&[0; 8]); // flags, invalidation bit position
    d.extend_from_slice(&[0; 4]); // precision, reserved, attachment count
    d.extend_from_slice(&[0; 48]); // ranges / limits
    d
}

fn cg_data(record_id: u64, cycles: u64, samples: u32) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&record_id.to_le_bytes());
    d.extend_from_slice(&cycles.to_le_bytes());
    d.extend_from_slice(&[0; 8]);
    d.extend_from_slice(&samples.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d
}

/// Variations of [`build_unsorted`] for malformed-input tests.
struct Unsorted {
    /// Byte offset of `Fast/Time`.
    fast_time_offset: u32,
    /// Bit count of `Fast/A`.
    a_bits: u32,
    /// Extra bytes appended to the record stream.
    trailing: Vec<u8>,
    /// Add a VLSD storage channel group (record id 3).
    vlsd_group: bool,
    /// Embedded attachment payload (`##AT` linked from the header).
    attachment: Option<Vec<u8>>,
    /// Add a `##FH` file-history entry.
    history: bool,
}

impl Default for Unsorted {
    fn default() -> Self {
        Self { fast_time_offset: 0, a_bits: 8, trailing: Vec::new(), vlsd_group: false, attachment: None, history: false }
    }
}

/// Unsorted data group (1-byte record ids) holding two channel groups:
/// `Fast` (id 1: Time f64 + A u8) and `Rare` (id 2: Time f64 + B u32).
fn build_unsorted() -> Vec<u8> {
    build_unsorted_with(&Unsorted::default())
}

fn build_unsorted_with(opts: &Unsorted) -> Vec<u8> {
    let mut f = vec![0u8; 64 + 104];
    f[0..8].copy_from_slice(b"MDF     ");
    f[8..16].copy_from_slice(b"4.10    ");
    f[16..24].copy_from_slice(b"handmade");
    f[28..30].copy_from_slice(&410u16.to_le_bytes());
    let mut push = |f: &mut Vec<u8>, b: Vec<u8>| -> u64 {
        let at = f.len() as u64;
        f.extend_from_slice(&b);
        at
    };
    let txt = |s: &str| {
        let mut d = s.as_bytes().to_vec();
        d.push(0);
        raw_block(b"##TX", &[], &d)
    };
    let names: Vec<u64> = ["Fast", "Rare", "Time", "A", "B"].iter().map(|s| push(&mut f, txt(s))).collect();
    let fast_a = push(&mut f, raw_block(b"##CN", &[0, 0, names[3], 0, 0, 0, 0, 0], &cn_data(0, 0, 0, 8, opts.a_bits)));
    let fast_t = push(
        &mut f,
        raw_block(b"##CN", &[fast_a, 0, names[2], 0, 0, 0, 0, 0], &cn_data(2, 1, 4, opts.fast_time_offset, 64)),
    );
    let rare_b = push(&mut f, raw_block(b"##CN", &[0, 0, names[4], 0, 0, 0, 0, 0], &cn_data(0, 0, 0, 8, 32)));
    let rare_t = push(&mut f, raw_block(b"##CN", &[rare_b, 0, names[2], 0, 0, 0, 0, 0], &cn_data(2, 1, 4, 0, 64)));

    let mut records = Vec::new();
    let (mut fast, mut rare) = (0u64, 0u64);
    for i in 0..30u64 {
        let t = i as f64 * 0.5;
        if i % 3 == 2 {
            records.push(2u8);
            records.extend_from_slice(&t.to_le_bytes());
            records.extend_from_slice(&(1000 + i as u32).to_le_bytes());
            rare += 1;
        } else {
            records.push(1u8);
            records.extend_from_slice(&t.to_le_bytes());
            records.push(i as u8);
            fast += 1;
        }
    }
    records.extend_from_slice(&opts.trailing);
    let vlsd_cg = if opts.vlsd_group {
        let mut d = cg_data(3, 0, 0);
        d[16] = 1; // VLSD channel group
        push(&mut f, raw_block(b"##CG", &[0, 0, 0, 0, 0, 0], &d))
    } else {
        0
    };
    let rare_cg = push(&mut f, raw_block(b"##CG", &[vlsd_cg, rare_t, names[1], 0, 0, 0], &cg_data(2, rare, 12)));
    let fast_cg = push(&mut f, raw_block(b"##CG", &[rare_cg, fast_t, names[0], 0, 0, 0], &cg_data(1, fast, 9)));
    let dt = push(&mut f, raw_block(b"##DT", &[], &records));
    let mut dg_data = vec![1u8];
    dg_data.extend_from_slice(&[0; 7]);
    let dg = push(&mut f, raw_block(b"##DG", &[0, fast_cg, dt, 0], &dg_data));
    let fh = if opts.history {
        let md = push(&mut f, txt("<FHcomment><TX>source history</TX></FHcomment>"));
        push(&mut f, raw_block(b"##FH", &[0, md], &[0; 16]))
    } else {
        0
    };
    let at = match &opts.attachment {
        Some(payload) => {
            let name = push(&mut f, txt("data.bin"));
            let mut d = vec![0u8; 40];
            d[0] = 1; // embedded
            d[24..32].copy_from_slice(&(payload.len() as u64).to_le_bytes());
            d[32..40].copy_from_slice(&(payload.len() as u64).to_le_bytes());
            d.extend_from_slice(payload);
            push(&mut f, raw_block(b"##AT", &[0, name, 0, 0], &d))
        }
        None => 0,
    };
    let mut hd_data = vec![0u8; 32];
    hd_data[0..8].copy_from_slice(&1_700_000_000_000_000_000u64.to_le_bytes());
    let hd = raw_block(b"##HD", &[dg, fh, 0, at, 0, 0], &hd_data);
    f[64..64 + 104].copy_from_slice(&hd);
    f
}

#[test]
fn unsorted_input_is_sorted_into_columns() -> Result<(), MdfError> {
    let (bytes, report) = to_column_oriented_bytes(&build_unsorted(), &ColumnOptions::default())?;
    assert_eq!(report.source_groups, 2);
    assert_eq!(report.time_groups, 2);
    assert_eq!(report.signal_groups, 2);
    let mdf = MDF::from_bytes(bytes)?;
    let a = mdf.signal_in("Fast", "A")?.unwrap();
    let b = mdf.signal_in("Rare", "B")?.unwrap();
    let fast_idx: Vec<u64> = (0..30).filter(|i| i % 3 != 2).collect();
    let rare_idx: Vec<u64> = (0..30).filter(|i| i % 3 == 2).collect();
    assert_eq!(a.values, fast_idx.iter().map(|&i| Some(DecodedValue::UnsignedInteger(i))).collect::<Vec<_>>());
    assert_eq!(a.timestamps, fast_idx.iter().map(|&i| i as f64 * 0.5).collect::<Vec<_>>());
    assert_eq!(b.values, rare_idx.iter().map(|&i| Some(DecodedValue::UnsignedInteger(1000 + i))).collect::<Vec<_>>());
    assert_eq!(b.timestamps, rare_idx.iter().map(|&i| i as f64 * 0.5).collect::<Vec<_>>());
    assert_eq!(mdf.start_time_ns(), Some(1_700_000_000_000_000_000));
    Ok(())
}

#[test]
fn compressed_input_is_rejected_cleanly() {
    let mut bytes = build_unsorted();
    // Turn the ##DT into a ##DZ.
    let pos = bytes.windows(4).position(|w| w == b"##DT").unwrap();
    bytes[pos + 3] = b'Z';
    let err = to_column_oriented_bytes(&bytes, &ColumnOptions::default()).unwrap_err();
    assert!(err.to_string().contains("compressed"), "{err}");
}

#[test]
fn malformed_channel_offsets_are_skipped_not_panicking() -> Result<(), MdfError> {
    // Byte offset / bit count near u32::MAX used to overflow the range check.
    let bytes = build_unsorted_with(&Unsorted { fast_time_offset: 0xFFFF_FFF8, a_bits: u32::MAX, ..Default::default() });
    let (out, report) = to_column_oriented_bytes(&bytes, &ColumnOptions::default())?;
    let skipped = report.skipped_channels.join("\n");
    assert!(skipped.contains("Fast/Time: lies outside the record"), "{skipped}");
    assert!(skipped.contains("Fast/A: lies outside the record"), "{skipped}");
    let mdf = MDF::from_bytes(out)?;
    assert_eq!(mdf.signal_in("Rare", "B")?.unwrap().values.len(), 10);
    Ok(())
}

#[test]
fn truncated_records_and_corrupt_vlsd_lengths_error() {
    // A partial record at the end of the data.
    let bytes = build_unsorted_with(&Unsorted { trailing: vec![1, 0, 0], ..Default::default() });
    let err = to_column_oriented_bytes(&bytes, &ColumnOptions::default()).unwrap_err();
    assert!(err.to_string().contains("inside a record"), "{err}");
    // A VLSD record claiming ~4 GiB: an error, not an attempt to buffer it.
    let bytes = build_unsorted_with(&Unsorted {
        trailing: vec![3, 0xff, 0xff, 0xff, 0xff, 1, 2, 3],
        vlsd_group: true,
        ..Default::default()
    });
    let err = to_column_oriented_bytes(&bytes, &ColumnOptions::default()).unwrap_err();
    assert!(err.to_string().contains("inside a record"), "{err}");
}

#[test]
fn attachments_and_history_are_copied_and_file_size_is_exact() -> Result<(), MdfError> {
    // Odd-sized payload larger than one read chunk, so the file ends unaligned
    // and the payload is streamed in several pieces.
    let payload: Vec<u8> = (0..200_003u32).map(|i| (i % 251) as u8).collect();
    let source = build_unsorted_with(&Unsorted { attachment: Some(payload.clone()), history: true, ..Default::default() });
    let dir = tempfile::tempdir()?;
    let (input, output) = (dir.path().join("in.mf4"), dir.path().join("out.mf4"));
    std::fs::write(&input, &source)?;
    let options = ColumnOptions { read_chunk_size: 64 * 1024, ..Default::default() };
    let report = to_column_oriented(&input, &output, &options)?;
    let out = std::fs::read(&output)?;
    assert_eq!(out.len() as u64, report.file_size, "on-disk size matches the report");
    let (mem, mem_report) = to_column_oriented_bytes(&source, &options)?;
    assert_eq!(mem.len() as u64, mem_report.file_size);
    assert_eq!(mem.len(), out.len());

    let u64_at = |o: u64| u64::from_le_bytes(out[o as usize..o as usize + 8].try_into().unwrap());
    // ##HD is always 104 bytes.
    assert_eq!(&out[64..68], b"##HD");
    assert_eq!(u64_at(72), 104);
    // History: the source entry, then ours.
    let fh = u64_at(64 + 24 + 8);
    assert_eq!(&out[fh as usize..fh as usize + 4], b"##FH");
    let ours = u64_at(fh + 24);
    assert_eq!(&out[ours as usize..ours as usize + 4], b"##FH");
    assert_eq!(u64_at(ours + 24), 0);
    // Attachment: header, links and payload intact.
    let at = u64_at(64 + 24 + 24);
    assert_eq!(&out[at as usize..at as usize + 4], b"##AT");
    assert!(at >= report.metadata_size, "attachments go after the data");
    let name = u64_at(at + 24 + 8);
    assert_eq!(&out[name as usize + 24..name as usize + 32], b"data.bin");
    let data = (at + 24 + 4 * 8 + 40) as usize;
    assert_eq!(&out[data..data + payload.len()], &payload[..]);
    assert_eq!(u64_at(at + 8), (24 + 4 * 8 + 40 + payload.len()) as u64);
    // Still a valid file.
    MDF::from_bytes(out)?.signal_in("Fast", "A")?.unwrap();
    Ok(())
}

#[test]
fn failed_conversion_removes_the_partial_output() -> Result<(), MdfError> {
    let dir = tempfile::tempdir()?;
    let (input, output) = (dir.path().join("in.mf4"), dir.path().join("out.mf4"));
    std::fs::write(&input, build_unsorted_with(&Unsorted { trailing: vec![1], ..Default::default() }))?;
    assert!(to_column_oriented(&input, &output, &ColumnOptions::default()).is_err());
    assert!(!output.exists());
    Ok(())
}

/// Sorted 4.20 source with three data groups:
/// * `T` — Time (f64, unit "s"), bus-event flag set
/// * `U` — Time (f64, unit `u_unit`, same timestamps as T), V (u8)
/// * `S` — no local master, remote master → T's group; X (u8), Y (sync
///   channel), Z (u16 structure with a ##CN child); `s_records` records
///
/// The header carries a 40-byte data section (longer than MDF 4's 32).
fn build_remote(u_unit: &str, s_records: u64) -> Vec<u8> {
    const N: u64 = 10;
    let mut f = vec![0u8; 64 + 24 + 48 + 40];
    f[0..8].copy_from_slice(b"MDF     ");
    f[8..16].copy_from_slice(b"4.20    ");
    f[16..24].copy_from_slice(b"handmade");
    f[28..30].copy_from_slice(&420u16.to_le_bytes());
    let push = |f: &mut Vec<u8>, b: Vec<u8>| -> u64 {
        let at = f.len() as u64;
        f.extend_from_slice(&b);
        at
    };
    let txt = |s: &str| {
        let mut d = s.as_bytes().to_vec();
        d.push(0);
        raw_block(b"##TX", &[], &d)
    };
    let tx: Vec<u64> = ["T", "U", "S", "Time", "s", u_unit, "V", "X", "Y", "Z", "Zc"]
        .iter()
        .map(|s| push(&mut f, txt(s)))
        .collect();
    let cn = |links: [u64; 8], d: Vec<u8>| raw_block(b"##CN", &links, &d);

    let t_time = push(&mut f, cn([0, 0, tx[3], 0, 0, 0, tx[4], 0], cn_data(2, 1, 4, 0, 64)));
    let mut d = cg_data(0, N, 8);
    d[16] = 0b10; // bus event group
    let cg_t = push(&mut f, raw_block(b"##CG", &[0, t_time, tx[0], 0, 0, 0], &d));
    let times: Vec<u8> = (0..N).flat_map(|i| (i as f64 * 0.1).to_le_bytes()).collect();
    let dt_t = push(&mut f, raw_block(b"##DT", &[], &times));

    let u_v = push(&mut f, cn([0, 0, tx[6], 0, 0, 0, 0, 0], cn_data(0, 0, 0, 8, 8)));
    let u_time = push(&mut f, cn([u_v, 0, tx[3], 0, 0, 0, tx[5], 0], cn_data(2, 1, 4, 0, 64)));
    let cg_u = push(&mut f, raw_block(b"##CG", &[0, u_time, tx[1], 0, 0, 0], &cg_data(0, N, 9)));
    let recs: Vec<u8> = (0..N).flat_map(|i| [(i as f64 * 0.1).to_le_bytes().to_vec(), vec![i as u8]].concat()).collect();
    let dt_u = push(&mut f, raw_block(b"##DT", &[], &recs));

    let zc = push(&mut f, cn([0, 0, tx[10], 0, 0, 0, 0, 0], cn_data(0, 0, 0, 2, 8)));
    let s_z = push(&mut f, cn([0, zc, tx[9], 0, 0, 0, 0, 0], cn_data(0, 0, 0, 2, 16)));
    let s_y = push(&mut f, cn([s_z, 0, tx[8], 0, 0, 0, 0, 0], cn_data(4, 1, 0, 1, 8)));
    let s_x = push(&mut f, cn([s_y, 0, tx[7], 0, 0, 0, 0, 0], cn_data(0, 0, 0, 0, 8)));
    let mut d = cg_data(0, s_records, 4);
    d[16] = 0b1000; // remote master
    let cg_s = push(&mut f, raw_block(b"##CG", &[0, s_x, tx[2], 0, 0, 0, cg_t], &d));
    let recs: Vec<u8> = (0..s_records).flat_map(|i| [i as u8 + 100, 0, 0, 0]).collect();
    let dt_s = push(&mut f, raw_block(b"##DT", &[], &recs));

    let dg_s = push(&mut f, raw_block(b"##DG", &[0, cg_s, dt_s, 0], &[0; 8]));
    let dg_u = push(&mut f, raw_block(b"##DG", &[dg_s, cg_u, dt_u, 0], &[0; 8]));
    let dg_t = push(&mut f, raw_block(b"##DG", &[dg_u, cg_t, dt_t, 0], &[0; 8]));
    let mut hd_data = vec![0u8; 40];
    hd_data[0..8].copy_from_slice(&1_700_000_000_000_000_000u64.to_le_bytes());
    let hd = raw_block(b"##HD", &[dg_t, 0, 0, 0, 0, 0], &hd_data);
    f[64..64 + hd.len()].copy_from_slice(&hd);
    f
}

#[test]
fn remote_master_structure_and_sync_channels() -> Result<(), MdfError> {
    let (out, report) = to_column_oriented_bytes(&build_remote("s", 10), &ColumnOptions::default())?;
    // T and U share a time axis; S reuses T's through its remote master.
    assert_eq!(report.time_groups, 1, "{report:?}");
    assert_eq!(report.shared_time_groups, 1, "{report:?}");
    assert_eq!(report.signal_groups, 2, "{report:?}");
    let skipped = report.skipped_channels.join("\n");
    assert!(skipped.contains("S/Y: synchronization"), "{skipped}");
    assert!(skipped.contains("S/Z: structure"), "{skipped}");
    assert_eq!(report.skipped_channels.len(), 2, "{skipped}");
    // ##HD written as the 104-byte MDF 4 block.
    assert_eq!(u64::from_le_bytes(out[72..80].try_into().unwrap()), 104);

    let mdf = MDF::from_bytes(out)?;
    assert_eq!(mdf.start_time_ns(), Some(1_700_000_000_000_000_000));
    let x = mdf.signal_in("S", "X")?.unwrap();
    assert_eq!(x.values, (0..10u64).map(|i| Some(DecodedValue::UnsignedInteger(100 + i))).collect::<Vec<_>>());
    assert_eq!(x.timestamps, (0..10).map(|i| i as f64 * 0.1).collect::<Vec<_>>());
    for g in mdf.channel_groups() {
        assert_eq!(g.raw_channel_group().block.flags & 0b110, 0, "bus-event flags masked");
    }

    // Different master units: no sharing.
    let (_, report) = to_column_oriented_bytes(&build_remote("ms", 10), &ColumnOptions::default())?;
    assert_eq!(report.time_groups, 2, "{report:?}");
    assert_eq!(report.shared_time_groups, 0, "{report:?}");

    // Remote master with a different record count: written without a master.
    let (out, report) = to_column_oriented_bytes(&build_remote("s", 5), &ColumnOptions::default())?;
    assert!(report.skipped_channels.iter().any(|s| s.contains("S: remote master dropped")), "{report:?}");
    let mdf = MDF::from_bytes(out)?;
    let x = mdf.signal_in("S", "X")?.unwrap();
    assert_eq!(x.values.len(), 5);
    assert!(x.timestamps.is_empty());
    Ok(())
}

#[test]
fn stale_cycle_count_still_reads_correctly_via_range_reader() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let expected = MdfIndex::from_file(f.column.to_str().unwrap())?.read_in("Engine", "Speed")?;
    let cg_addr = {
        let mdf = MDF::from_file(f.column.to_str().unwrap())?;
        let groups = mdf.channel_groups();
        let g = groups
            .iter()
            .find(|g| g.channels()[0].name().ok().flatten().as_deref() == Some("Speed"))
            .unwrap();
        assert!(g.raw_channel_group().block.header.links_nr == 7);
        g.raw_channel_group().addr as usize
    };
    // A small and a large under-count: within the last fragment, and beyond it.
    for lower in [3u64, FAST as u64 / 2] {
        let mut bytes = std::fs::read(&f.column)?;
        let at = cg_addr + 24 + 7 * 8 + 8; // cg_cycle_count
        let cycles = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        bytes[at..at + 8].copy_from_slice(&(cycles - lower).to_le_bytes());
        let path = f.column.with_file_name(format!("stale_{lower}.mf4"));
        std::fs::write(&path, &bytes)?;
        let path = path.to_str().unwrap();
        let mut reader = mf4_rs::index::FileRangeReader::new(path)?;
        let mut index = MdfIndex::from_range_reader(&mut reader, bytes.len() as u64)?;
        index.set_file(path);
        let sig = index.read_in("Engine", "Speed")?;
        assert_eq!(sig.values, expected.values, "lowered by {lower}");
        assert_eq!(sig.timestamps, expected.timestamps, "lowered by {lower}");
    }
    Ok(())
}

#[test]
fn channel_in_searches_every_same_named_group() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let index = MdfIndex::from_file(f.column.to_str().unwrap())?;
    // The first "Engine" group is the time group; Speed lives in a later one.
    assert_eq!(index.channel_in("Engine", "Speed").and_then(|c| c.name.as_deref()), Some("Speed"));
    assert!(index.channel_in("Engine", "Pressure").is_none());
    Ok(())
}

#[test]
fn cut_and_merge_reject_remote_master_files() -> Result<(), MdfError> {
    let f = fixture(&small_blocks())?;
    let (col, src) = (f.column.to_str().unwrap(), f.source.to_str().unwrap());
    let out = f.column.with_file_name("out.mf4");
    let out = out.to_str().unwrap();
    let err = mf4_rs::cut::cut_mdf_by_time(col, out, 0.0, 1.0).unwrap_err();
    assert!(err.to_string().contains("remote master"), "{err}");
    let err = mf4_rs::merge::merge_files(out, src, col).unwrap_err();
    assert!(err.to_string().contains("remote master"), "{err}");
    let err = mf4_rs::merge::merge_files(out, col, src).unwrap_err();
    assert!(err.to_string().contains("remote master"), "{err}");
    // Row-oriented files are unaffected.
    mf4_rs::cut::cut_mdf_by_time(src, out, 0.0, 1.0)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Real HTTP: MdfIndex::from_url + lazy reads, counting requests at the server
// ---------------------------------------------------------------------------

#[cfg(feature = "http")]
mod http {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    struct Server {
        url: String,
        requests: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// Minimal single-range HTTP server that counts every request.
    fn serve(path: &Path) -> Server {
        let bytes = std::fs::read(path).unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.mf4", server.server_addr().to_ip().unwrap());
        let requests = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (req_count, stop_flag) = (requests.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(20)) else { continue };
                req_count.fetch_add(1, Ordering::SeqCst);
                let total = bytes.len() as u64;
                let range = req
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("range"))
                    .and_then(|h| {
                        let (s, e) = h.value.as_str().strip_prefix("bytes=")?.split_once('-')?;
                        let s: u64 = s.parse().ok()?;
                        let e: u64 = if e.is_empty() { total - 1 } else { e.parse().ok()? };
                        (s < total).then_some((s, e.min(total - 1)))
                    });
                let resp = match range {
                    Some((s, e)) => tiny_http::Response::from_data(bytes[s as usize..=e as usize].to_vec())
                        .with_status_code(206)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Content-Range"[..], format!("bytes {s}-{e}/{total}").as_bytes())
                                .unwrap(),
                        ),
                    None => tiny_http::Response::from_data(bytes.clone()),
                };
                let _ = req.respond(resp);
            }
        });
        Server { url, requests, stop, handle: Some(handle) }
    }

    #[test]
    fn from_url_build_and_signal_reads() -> Result<(), MdfError> {
        let f = fixture(&small_blocks())?;
        let expected = signals_via_mdf(&f.source)?;
        let server = serve(&f.column);

        let index = MdfIndex::from_url(&server.url)?;
        let build = server.requests.load(Ordering::SeqCst);

        let mut per_read = Vec::new();
        for (g, c) in [("Engine", "Speed"), ("Engine", "Gear"), ("Body", "Temp"), ("Body", "Name"), ("Untimed", "Counter")] {
            let before = server.requests.load(Ordering::SeqCst);
            let sig = index.read_in(g, c)?;
            per_read.push((format!("{g}/{c}"), server.requests.load(Ordering::SeqCst) - before));
            let e = &expected[&format!("{g}/{c}")];
            assert_eq!(sig.values, e.values, "{g}/{c}");
            assert_eq!(sig.timestamps, e.timestamps, "{g}/{c}");
        }
        eprintln!("from_url build: {build} requests (size probe + metadata); per read: {per_read:?}");
        // Size probe + one metadata prefix fetch.
        assert!(build <= 2, "build took {build} requests");
        for (name, n) in &per_read {
            // values + shared time channel (+1 for a lazily resolved conversion,
            // +1 for the ##SD stream of a VLSD channel); no timestamps when untimed.
            let budget = match name.as_str() {
                "Engine/Gear" | "Body/Name" => 3,
                "Untimed/Counter" => 1,
                _ => 2,
            };
            assert!(*n <= budget, "{name}: {n} requests (budget {budget})");
        }
        Ok(())
    }
}
