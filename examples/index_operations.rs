//! Lazy reads through an `MdfIndex`.
//!
//! An index holds only metadata. It remembers a data source (file path or
//! URL) and fetches just the bytes each read needs; it has the same reading
//! API as `Mdf`. The same calls exist in Python and TypeScript — see
//! docs/API.md.

use mf4_rs::{DecodedValue, Mdf, MdfError, MdfIndex, MdfWriter, ReadOptions};

fn main() -> Result<(), MdfError> {
    let mdf_file = "index_example.mf4";
    let index_file = "index_example.json";
    write_sample(mdf_file)?;

    // Build an index (reads metadata only) and save it as JSON.
    let index = MdfIndex::from_file(mdf_file)?;
    index.save(index_file)?;
    println!("indexed {} bytes; source = {:?}", index.file_size(), index.source());

    // Later: load the index. The source is not serialised — re-attach it.
    // A URL works the same way (with the `http` feature): range requests.
    let mut index = MdfIndex::load(index_file)?;
    index.set_source(mdf_file);

    // The same reading API as `Mdf`.
    for group in index.groups()? {
        println!("{:?}: {} records, channels {:?}", group.name, group.record_count, group.channel_names);
    }
    let temperature = index.read("Temperature")?;
    println!("Temperature: {} samples, first {:?}", temperature.len(), temperature.values.first());
    let window = index.values_with("Temperature", &ReadOptions::new().time(Some(1.0), Some(1.5)))?;
    println!("Temperature between 1.0s and 1.5s: {window:?}");
    println!("time range: {:?}", index.time_range("Temperature", None)?);
    println!("conversion: {:?}", index.conversion("Temperature", None)?);

    // Both readers agree.
    let direct = Mdf::from_file(mdf_file)?;
    assert_eq!(direct.values("Temperature")?, index.values("Temperature")?);

    // Power users: the byte ranges a channel occupies, for your own range requests.
    println!("byte ranges of Temperature: {:?}", index.byte_ranges("Temperature", None)?);
    println!("…for records 10..20: {:?}", index.byte_ranges_for_records("Temperature", 10, 10, None)?);

    // Custom byte sources (S3, caches, …) implement `ByteRangeReader`;
    // `open` binds one and offers the same read methods.
    let mut reader = index.open(mf4_rs::index::FileRangeReader::new(mdf_file)?);
    println!("via a custom reader: {} samples", reader.values("Temperature")?.len());

    std::fs::remove_file(mdf_file).ok();
    std::fs::remove_file(index_file).ok();
    Ok(())
}

fn write_sample(path: &str) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;
    let group = writer.add_channel_group(Some("Engine"))?;
    writer.add_time_channel(&group, "Time")?;
    let temperature = writer.add_float_channel(&group, "Temperature")?;
    writer.add_linear_conversion(&temperature, -40.0, 0.5)?;
    writer.start_data_block(&group)?;
    for i in 0..100 {
        writer.write_record(&group, &[DecodedValue::Float(i as f64 * 0.1), DecodedValue::Float(120.0 + i as f64)])?;
    }
    writer.finish_data_block(&group)?;
    writer.finalize()
}
