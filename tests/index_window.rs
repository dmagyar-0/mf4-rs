//! Record- and time-window reads through the index, master self-timestamps,
//! virtual masters and channel comments.

use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, FileRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Counts the bytes requested through it.
struct Counting<R> {
    inner: R,
    bytes: u64,
}

impl<R: ByteRangeReader<Error = MdfError>> ByteRangeReader for Counting<R> {
    type Error = MdfError;
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        self.bytes += length;
        self.inner.read_range(offset, length)
    }
}

/// `n` records of `Time = i * 0.01` and `V = i`, split over several data
/// blocks (8-byte records, ~4 MB blocks) when `n` is large.
fn write_file(name: &str, n: u64) -> String {
    let path = std::env::temp_dir().join(name);
    let path = path.to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(None, |_| {}).unwrap();
    let t = w
        .add_channel(&cg, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.name = Some("Time".into());
            ch.bit_count = 64;
        })
        .unwrap();
    w.set_time_channel(&t).unwrap();
    w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("V".into());
        ch.bit_count = 64;
    })
    .unwrap();
    w.start_data_block_for_cg(&cg, 0).unwrap();
    for i in 0..n {
        w.write_record(&cg, &[DecodedValue::Float(i as f64 * 0.01), DecodedValue::UnsignedInteger(i)])
            .unwrap();
    }
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();
    path
}

fn uints(values: &[Option<DecodedValue>]) -> Vec<u64> {
    values
        .iter()
        .map(|v| match v {
            Some(DecodedValue::UnsignedInteger(u)) => *u,
            other => panic!("unexpected {:?}", other),
        })
        .collect()
}

#[test]
fn record_window_matches_full_read_across_blocks() {
    let n = 600_000; // 16-byte records -> several 4 MB ##DT blocks
    let path = write_file("index_window_records.mf4", n);
    let index = MdfIndex::from_file(&path).unwrap();
    assert!(index.groups()[0].data_blocks.len() > 1);
    let mut r = index.open(FileRangeReader::new(&path).unwrap());

    let start = 262_000; // straddles the first block boundary
    let sig = r.signal_records("V", start, 1000).unwrap();
    assert_eq!(uints(&sig.values), (start..start + 1000).collect::<Vec<_>>());
    assert_eq!(sig.timestamps.len(), 1000);
    assert!((sig.timestamps[0] - start as f64 * 0.01).abs() < 1e-9);

    assert!(r.signal_records("V", n - 1, 2).is_err());
    assert_eq!(r.signal_records("V", n, 0).unwrap().values.len(), 0);
}

#[test]
fn time_window_reads_only_a_small_part() {
    let n = 600_000;
    let path = write_file("index_window_time.mf4", n);
    let index = MdfIndex::from_file(&path).unwrap();
    let mut r = index.open(Counting { inner: FileRangeReader::new(&path).unwrap(), bytes: 0 });

    let sig = r.signal_between("V", Some(100.0), Some(100.5)).unwrap();
    // 0.01 s spacing: records 10000..=10050 (float rounding may shift an edge)
    let got = uints(&sig.values);
    assert!(got.first().unwrap().abs_diff(10_000) <= 1, "{:?}", got.first());
    assert!(got.last().unwrap().abs_diff(10_050) <= 1, "{:?}", got.last());
    for t in &sig.timestamps {
        assert!((100.0..=100.5).contains(t));
    }
    assert!(r.reader_mut().bytes < 64 * 1024, "read {} bytes", r.reader_mut().bytes);

    // Open-ended windows and an empty window.
    let tail = r.signal_between("V", Some(5999.9), None).unwrap();
    assert_eq!(uints(&tail.values).last(), Some(&(n - 1)));
    let head = r.signal_between("V", None, Some(0.025)).unwrap();
    assert_eq!(uints(&head.values), vec![0, 1, 2]);
    assert!(r.signal_between("V", Some(1e9), None).unwrap().values.is_empty());
    assert!(r.signal_between("V", Some(5.0), Some(4.0)).unwrap().values.is_empty());

    // Time window on the master itself (its values are the time axis).
    let m = r.signal_between("Time", Some(1.0), Some(1.02)).unwrap();
    assert_eq!(m.values.len(), 3);
    assert!(m.timestamps.is_empty());
}

#[test]
fn virtual_master_reads_record_index() {
    // A virtual master (cn_type 3) occupies no record bytes: its raw value is
    // the record index.
    let path = std::env::temp_dir().join("index_window_virtual.mf4");
    let path = path.to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(None, |_| {}).unwrap();
    let v = w
        .add_channel(&cg, None, |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.name = Some("V".into());
            ch.bit_count = 32;
        })
        .unwrap();
    w.add_channel(&cg, Some(&v), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("t".into());
        // The writer still reserves record bytes for it; a virtual channel's
        // bytes are ignored by readers.
        ch.channel_type = 3;
        ch.sync_type = 1;
    })
    .unwrap();
    w.start_data_block_for_cg(&cg, 0).unwrap();
    for i in 0..4u64 {
        w.write_record(&cg, &[DecodedValue::UnsignedInteger(i * 10), DecodedValue::UnsignedInteger(0)])
            .unwrap();
    }
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();

    let index = MdfIndex::from_file(&path).unwrap();
    assert!(index.groups()[0].channel("t").unwrap().is_master());
    let mut r = index.open(FileRangeReader::new(&path).unwrap());
    assert_eq!(r.values_f64("t").unwrap(), vec![0.0, 1.0, 2.0, 3.0]);
    let sig = r.signal("V").unwrap();
    assert_eq!(sig.timestamps, vec![0.0, 1.0, 2.0, 3.0]);
    assert_eq!(uints(&r.signal_between("V", Some(1.0), Some(2.0)).unwrap().values), vec![10, 20]);

    let mdf = MDF::from_file(&path).unwrap();
    assert_eq!(mdf.signal("V").unwrap().unwrap().timestamps, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn channel_comment_is_indexed_locally_and_remotely() {
    use mf4_rs::blocks::text_block::TextBlock;
    let path = std::env::temp_dir().join("index_window_comment.mf4");
    let path = path.to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(None, |_| {}).unwrap();
    let t = w
        .add_channel(&cg, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.name = Some("Time".into());
            ch.bit_count = 64;
        })
        .unwrap();
    w.set_time_channel(&t).unwrap();
    let tx = TextBlock::new("seconds since start").to_bytes().unwrap();
    w.write_block_with_id(&tx, "tx_time_comment").unwrap();
    w.update_block_link(&t, 80, "tx_time_comment").unwrap(); // cn_md_comment
    w.start_data_block_for_cg(&cg, 0).unwrap();
    w.write_record(&cg, &[DecodedValue::Float(0.0)]).unwrap();
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();

    let local = MdfIndex::from_file(&path).unwrap();
    assert_eq!(local.channel("Time").unwrap().comment.as_deref(), Some("seconds since start"));

    let size = std::fs::metadata(&path).unwrap().len();
    let mut reader = FileRangeReader::new(&path).unwrap();
    let remote = MdfIndex::from_range_reader(&mut reader, size).unwrap();
    assert_eq!(remote.channel("Time").unwrap().comment.as_deref(), Some("seconds since start"));

    // Old JSON without the field still loads.
    let mut v: serde_json::Value = serde_json::from_str(&local.to_json().unwrap()).unwrap();
    v["channel_groups"][0]["channels"][0].as_object_mut().unwrap().remove("comment").unwrap();
    let json = v.to_string();
    assert!(MdfIndex::from_json(&json).unwrap().channel("Time").unwrap().comment.is_none());
}

#[test]
fn writer_linear_conversion_is_applied() {
    let path = std::env::temp_dir().join("index_window_linear.mf4");
    let path = path.to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(None, |_| {}).unwrap();
    let v = w
        .add_channel(&cg, None, |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.name = Some("V".into());
            ch.bit_count = 16;
        })
        .unwrap();
    w.add_linear_conversion(-40.0, 0.5, Some(&v)).unwrap();
    w.start_data_block_for_cg(&cg, 0).unwrap();
    w.write_record(&cg, &[DecodedValue::UnsignedInteger(100)]).unwrap();
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();

    let mdf = MDF::from_file(&path).unwrap();
    let group = &mdf.channel_groups()[0];
    let ch = &group.channels()[0];
    assert_eq!(ch.values_as_f64().unwrap(), vec![10.0]);
    assert_eq!(ch.conversion().unwrap().unwrap().cc_val, vec![-40.0, 0.5]);
}
