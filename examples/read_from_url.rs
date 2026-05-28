//! Read an MDF file from an HTTP URL using range requests.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example read_from_url --features http -- <URL>
//! ```
//!
//! The reader issues HTTP `Range:` requests for metadata and sample data
//! blocks, never downloading the full file. `CachingRangeReader` collapses
//! the many small metadata reads into a handful of round-trips while the
//! index is being built; reads of large sample-data ranges then bypass the
//! cache.
use mf4_rs::error::MdfError;
use mf4_rs::index::{CachingRangeReader, HttpRangeReader, MdfIndex};

fn main() -> Result<(), MdfError> {
    let url = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: read_from_url <URL>");
        std::process::exit(1);
    });

    // 1. Probe the file size with a HEAD request.
    let mut http = HttpRangeReader::new(&url)?;
    let file_size = http.probe_size()?;
    println!("file size: {} bytes", file_size);

    // 2. Build the index over a caching wrapper so metadata reads stay cheap.
    let mut cached = CachingRangeReader::new(http);
    let index = MdfIndex::from_range_reader(&mut cached, file_size)?;
    println!(
        "indexed {} channel group(s) using {} HTTP request(s)",
        index.channel_groups.len(),
        cached.underlying_requests(),
    );

    // 3. Bypass the cache for value reads — sample data is read once.
    cached.set_bypass(true);

    // 4. Read the first channel of the first group as f64.
    let Some(group) = index.channel_groups.first() else {
        println!("file has no channel groups");
        return Ok(());
    };
    let Some(channel) = group.channels.first() else {
        println!("first channel group has no channels");
        return Ok(());
    };
    let name = channel.name.as_deref().unwrap_or("<unnamed>");

    let values = index.read_channel_values(0, 0, &mut cached)?;
    println!("channel '{}' — {} samples", name, values.len());
    for (i, value) in values.iter().take(5).enumerate() {
        println!("  [{}] {:?}", i, value);
    }

    Ok(())
}
