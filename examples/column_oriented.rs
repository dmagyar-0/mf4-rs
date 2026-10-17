//! Convert an MDF 4 file into a column-oriented MDF 4.20 file optimised for
//! reading single signals from cloud storage.
//!
//! ```text
//! cargo run --release --example column_oriented -- input.mf4 output.mf4 [block_size_bytes]
//! ```

use mf4_rs::column::{to_column_oriented, ColumnOptions};
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <input.mf4> <output.mf4> [block_size_bytes]", args[0]);
        std::process::exit(2);
    }
    let mut options = ColumnOptions::default();
    if let Some(size) = args.get(3) {
        options.block_size = size
            .parse()
            .map_err(|_| MdfError::BlockSerializationError(format!("invalid block size '{size}'")))?;
    }

    let started = std::time::Instant::now();
    let report = to_column_oriented(&args[1], &args[2], &options)?;
    println!("converted in {:.2?}", started.elapsed());
    println!(
        "{} signal groups, {} time groups ({} source groups share another's time axis), {} data blocks",
        report.signal_groups, report.time_groups, report.shared_time_groups, report.data_blocks
    );
    println!(
        "metadata prefix: {} bytes of {} ({:.3}%)",
        report.metadata_size,
        report.file_size,
        100.0 * report.metadata_size as f64 / report.file_size.max(1) as f64
    );
    for skipped in &report.skipped_channels {
        println!("skipped {skipped}");
    }
    Ok(())
}
