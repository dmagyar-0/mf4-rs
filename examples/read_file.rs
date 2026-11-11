//! Read an MDF 4 file with the name-based reader API.
//!
//! Run `cargo run --example write_file` first to create `example.mf4`. The
//! same calls exist in Python and TypeScript — see docs/API.md.

use mf4_rs::{Mdf, MdfError, ReadOptions};

fn main() -> Result<(), MdfError> {
    let mdf = Mdf::from_file("example.mf4")?;
    println!("start time: {:?} ns", mdf.start_time_ns());

    // Metadata: groups and their channels.
    for group in mdf.groups()? {
        println!(
            "\ngroup {:?}: {} records, master {:?}",
            group.name, group.record_count, group.master_channel
        );
        for channel in &group.channels {
            println!(
                "  {:<12} {:<18} {:>2} bits  unit={:?} conversion={}",
                channel.name.as_deref().unwrap_or("<unnamed>"),
                channel.data_type,
                channel.bit_count,
                channel.unit,
                channel.has_conversion
            );
        }
    }

    // read() -> Signal: values (conversions applied) + the group's time axis.
    let temperature = mdf.read("Temperature", None)?;
    println!("\n{}: {} samples", temperature.name, temperature.len());
    for (t, v) in temperature.timestamps.iter().zip(&temperature.values).take(3) {
        println!("  t={t:.1}s  {v:?}");
    }

    // values() -> Vec<f64>: just the numbers.
    let speed = mdf.values("Speed", None)?;
    println!("Speed: {} samples, mean {:.2}", speed.len(), speed.iter().sum::<f64>() / speed.len() as f64);

    // Text conversions come back as strings through read().
    println!("Gear: {:?}", &mdf.read("Gear", None)?.values[..3]);

    // Narrow by group, and read only a time or record window.
    let window = mdf.read("Time", &ReadOptions::new().group("Vehicle").records(0, 5))?;
    println!("first 5 Vehicle times: {:?}", window.values_f64());
    let slice = mdf.values("Speed", &ReadOptions::new().time(Some(1.0), Some(1.05)))?;
    println!("Speed between 1.00s and 1.05s: {slice:?}");

    // Inspection.
    println!("Speed time range: {:?}", mdf.time_range("Speed", None)?);
    println!("Temperature conversion: {:?}", mdf.conversion("Temperature", None)?);

    // Unknown names: metadata lookups return None, data reads return an error.
    assert!(mdf.channel("Nope", None)?.is_none());
    assert!(matches!(mdf.read("Nope", None), Err(MdfError::ChannelNotFound { .. })));
    Ok(())
}
