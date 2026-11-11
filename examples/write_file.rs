//! Write an MDF 4 file with the name-based writer API.
//!
//! Creates `example.mf4`, which `read_file` and the Python / TypeScript
//! examples can read. The same calls exist in Python (`MdfWriter`) and
//! TypeScript (`MdfWriter`) — see docs/API.md.

use mf4_rs::{ColumnData, DecodedValue, Mdf, MdfError, MdfWriter};

fn main() -> Result<(), MdfError> {
    let mut writer = MdfWriter::new("example.mf4")?;
    writer.init_mdf_file()?;

    // One channel group with a time (master) channel and three data channels.
    let group = writer.add_channel_group(Some("Engine"))?;
    writer.set_channel_group_comment(&group, "Demo measurement")?;
    writer.add_time_channel(&group, "Time")?; // add the master first
    let temperature = writer.add_float_channel(&group, "Temperature")?;
    let gear = writer.add_int_channel(&group, "Gear")?;
    writer.add_string_channel(&group, "State")?;

    // Conversions turn raw values into physical ones when reading.
    writer.add_linear_conversion(&temperature, -40.0, 0.5)?; // raw -> °C
    writer.add_value_to_text_conversion(&gear, &[(0, "N"), (1, "D"), (2, "R")], "?")?;

    writer.start_data_block(&group)?;
    for i in 0..100u64 {
        writer.write_record(
            &group,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::Float(120.0 + i as f64),
                DecodedValue::UnsignedInteger(i % 3),
                DecodedValue::String(if i % 2 == 0 { "idle" } else { "running" }.to_string()),
            ],
        )?;
    }
    writer.finish_data_block(&group)?;

    // A second, purely numeric group written column by column (the fast path).
    let group = writer.add_channel_group(Some("Vehicle"))?;
    writer.add_time_channel(&group, "Time")?;
    writer.add_float_channel(&group, "Speed")?;
    let time: Vec<f64> = (0..1000).map(|i| i as f64 * 0.01).collect();
    let speed: Vec<f64> = time.iter().map(|t| 50.0 + 10.0 * t.sin()).collect();
    writer.start_data_block(&group)?;
    writer.write_columns(&group, &[ColumnData::F64(&time), ColumnData::F64(&speed)])?;
    writer.finish_data_block(&group)?;

    writer.finalize()?;

    // Read it back.
    let mdf = Mdf::from_file("example.mf4")?;
    for group in mdf.groups()? {
        println!("{:?}: {} records, channels {:?}", group.name, group.record_count, group.channel_names);
    }
    println!("first temperatures: {:?}", &mdf.values("Temperature")?[..3]);
    println!("Created example.mf4");
    Ok(())
}
