//! `ReadOptions::raw`: values as stored, without the channel's conversion.
//!
//! The shared golden test (`api_contract.rs`) covers the common cases in all
//! three languages; these add a master with its own conversion and a
//! range-reader index whose conversions are resolved lazily.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use mf4_rs::index::FileRangeReader;
use mf4_rs::{ByteRangeReader, DecodedValue, Mdf, MdfError, MdfIndex, MdfWriter, ReadOptions, Signal};

/// `Signal` has no `PartialEq`; compare its debug form.
fn same(a: &Signal) -> String {
    format!("{a:?}")
}

/// Ticks counted at 10 ms (`Time = 0.01 * tick`), plus `Speed = 1 + 2 * raw`
/// and `Gear` with a value-to-text table.
fn write_file(path: &Path) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let g = w.add_channel_group(Some("Engine"))?;
    let time = w.add_int_channel(&g, "Time")?;
    w.set_time_channel(&time)?;
    let speed = w.add_float_channel(&g, "Speed")?;
    let gear = w.add_int_channel(&g, "Gear")?;
    w.add_linear_conversion(&time, 0.0, 0.01)?;
    w.add_linear_conversion(&speed, 1.0, 2.0)?;
    w.add_value_to_text_conversion(&gear, &[(0, "N"), (1, "D")], "?")?;
    w.start_data_block(&g)?;
    for i in 0..10u64 {
        w.write_record(
            &g,
            &[
                DecodedValue::UnsignedInteger(i * 10),
                DecodedValue::Float(i as f64),
                DecodedValue::UnsignedInteger(i % 3),
            ],
        )?;
    }
    w.finish_data_block(&g)?;
    w.finalize()
}

#[test]
fn raw_skips_the_conversion_but_keeps_the_time_axis_physical() -> Result<(), MdfError> {
    let dir = tempfile::tempdir().map_err(MdfError::IOError)?;
    let path = dir.path().join("raw.mf4");
    write_file(&path)?;
    let path = path.to_str().unwrap();
    let mdf = Mdf::from_file(path)?;
    let index = MdfIndex::from_file(path)?;
    let raw = ReadOptions::new().raw(true);

    let speed = mdf.read("Speed", &raw)?;
    assert_eq!(speed.values_f64(), (0..10).map(f64::from).collect::<Vec<_>>());
    // Timestamps are the converted master: 0.0, 0.1, ..., not 0, 10, ...
    assert!((speed.timestamps[9] - 0.9).abs() < 1e-12);
    assert_eq!(mdf.read("Speed", None)?.values_f64()[9], 19.0);

    let gear = mdf.read("Gear", &raw)?;
    assert!(matches!(gear.values[2], Some(DecodedValue::UnsignedInteger(2))));
    assert!(matches!(mdf.read("Gear", None)?.values[2], Some(DecodedValue::String(ref t)) if t == "?"));

    // A time window selects on the physical axis, raw or not — also when the
    // raw channel is the master itself.
    let window = ReadOptions::new().time(Some(0.2), Some(0.4));
    let raw_window = window.clone().raw(true);
    assert_eq!(mdf.values("Speed", &raw_window)?, vec![2.0, 3.0, 4.0]);
    assert_eq!(mdf.values("Time", &raw_window)?, vec![20.0, 30.0, 40.0]);
    let time = mdf.read("Time", &raw_window)?;
    assert!(time.timestamps.is_empty());
    assert_eq!(time.values_f64(), vec![20.0, 30.0, 40.0]);
    assert_eq!(mdf.read("Time", &window)?.values_f64().len(), 3);

    // The index gives the same answers on every path.
    for (name, options) in [
        ("Speed", raw.clone()),
        ("Gear", raw.clone()),
        ("Time", raw.clone()),
        ("Speed", raw_window.clone()),
        ("Time", raw_window.clone()),
        ("Gear", ReadOptions::new().records(3, 4).raw(true)),
    ] {
        assert_eq!(same(&index.read(name, &options)?), same(&mdf.read(name, &options)?), "{name} {options:?}");
        assert_eq!(index.values(name, &options)?, mdf.values(name, &options)?, "{name} {options:?}");
        let mut reader = index.open_file(path)?;
        assert_eq!(same(&reader.read(name, &options)?), same(&mdf.read(name, &options)?), "{name} {options:?}");
        assert_eq!(reader.values(name, &options)?, mdf.values(name, &options)?, "{name} {options:?}");
    }
    Ok(())
}

/// Records every range read through it.
struct LoggingReader {
    inner: FileRangeReader,
    log: Rc<RefCell<Vec<(u64, u64)>>>,
}

impl ByteRangeReader for LoggingReader {
    type Error = MdfError;
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        self.log.borrow_mut().push((offset, length));
        self.inner.read_range(offset, length)
    }
}

#[test]
fn raw_reads_of_a_lazy_index_never_fetch_the_conversion() -> Result<(), MdfError> {
    let dir = tempfile::tempdir().map_err(MdfError::IOError)?;
    let path = dir.path().join("lazy.mf4");
    write_file(&path)?;
    let path = path.to_str().unwrap();
    let size = std::fs::metadata(path).map_err(MdfError::IOError)?.len();

    let log = Rc::new(RefCell::new(Vec::new()));
    let mut reader = LoggingReader { inner: FileRangeReader::new(path)?, log: log.clone() };
    let index = MdfIndex::from_range_reader(&mut reader, size)?;
    let speed = index.indexed_channel("Speed", None).unwrap();
    assert!(speed.conversion.is_none(), "a range-reader build defers conversions");
    let cc = speed.conversion_addr;
    assert_ne!(cc, 0);
    let touches_cc = |log: &[(u64, u64)]| log.iter().any(|&(o, l)| o <= cc && cc < o + l);

    let mdf = Mdf::from_file(path)?;
    let mut bound = index.open(reader);
    for options in [ReadOptions::new().raw(true), ReadOptions::new().records(2, 3).raw(true)] {
        log.borrow_mut().clear();
        assert_eq!(bound.values("Speed", &options)?, mdf.values("Speed", &options)?);
        assert_eq!(same(&bound.read("Speed", &options)?), same(&mdf.read("Speed", &options)?));
        assert!(!touches_cc(&log.borrow()), "raw read fetched the conversion block");
    }

    log.borrow_mut().clear();
    assert_eq!(bound.values("Speed", None)?, mdf.values("Speed", None)?);
    assert!(touches_cc(&log.borrow()), "a converted read resolves the conversion");
    Ok(())
}
