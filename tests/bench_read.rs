/// Detailed read performance benchmark for mf4-rs
/// Tests various channel types and record counts to establish baselines.
use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mf4rs_bench_{}.mf4", name))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Write a test file with N records of 4 x f64 channels
fn write_f64_file(path: &std::path::Path, n: usize) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&t)?;
    let a = w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("A".into());
        ch.bit_count = 64;
    })?;
    let b = w.add_channel(&cg, Some(&a), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("B".into());
        ch.bit_count = 64;
    })?;
    w.add_channel(&cg, Some(&b), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("C".into());
        ch.bit_count = 64;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..n {
        let v = i as f64 * 0.001;
        w.write_record(&cg, &[
            DecodedValue::Float(v),
            DecodedValue::Float(v * 2.0),
            DecodedValue::Float(v * 3.0),
            DecodedValue::Float(v * 4.0),
        ])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(())
}

/// Write a test file with N records of mixed types (u32 + f64 + i64 + u64)
fn write_mixed_file(path: &std::path::Path, n: usize) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let ch1 = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("counter".into());
        ch.bit_count = 32;
    })?;
    let ch2 = w.add_channel(&cg, Some(&ch1), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("measurement".into());
        ch.bit_count = 64;
    })?;
    let ch3 = w.add_channel(&cg, Some(&ch2), |ch| {
        ch.data_type = DataType::SignedIntegerLE;
        ch.name = Some("offset".into());
        ch.bit_count = 64;
    })?;
    w.add_channel(&cg, Some(&ch3), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("flags".into());
        ch.bit_count = 64;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..n {
        w.write_record(&cg, &[
            DecodedValue::UnsignedInteger(i as u64),
            DecodedValue::Float(i as f64 * 0.1),
            DecodedValue::SignedInteger(-(i as i64)),
            DecodedValue::UnsignedInteger(i as u64 * 1000),
        ])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(())
}

#[test]
fn bench_read_f64_100k() -> Result<(), MdfError> {
    let path = temp_path("bench_f64_100k");
    cleanup(&path);
    let n = 100_000usize;
    write_f64_file(&path, n)?;

    // Warmup - read file once
    let _ = MDF::from_file(path.to_str().unwrap())?;

    // Benchmark: read all channels
    let iterations = 5;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let mut total = 0usize;
        for group in mdf.channel_groups() {
            for channel in group.channels() {
                total += channel.values()?.len();
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(total, n * 4);
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    let best = times[0];
    eprintln!(
        "bench_read_f64_100k: median={:.4}s, best={:.4}s ({} records x 4 channels)",
        median.as_secs_f64(), best.as_secs_f64(), n,
    );
    eprintln!(
        "  throughput: {:.1}M values/sec (median)",
        (n * 4) as f64 / median.as_secs_f64() / 1_000_000.0,
    );

    cleanup(&path);
    Ok(())
}

#[test]
fn bench_read_f64_1m() -> Result<(), MdfError> {
    let path = temp_path("bench_f64_1m");
    cleanup(&path);
    let n = 1_000_000usize;
    write_f64_file(&path, n)?;

    let iterations = 3;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let mut total = 0usize;
        for group in mdf.channel_groups() {
            for channel in group.channels() {
                total += channel.values()?.len();
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(total, n * 4);
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    eprintln!(
        "bench_read_f64_1m: median={:.4}s ({} records x 4 channels)",
        median.as_secs_f64(), n,
    );
    eprintln!(
        "  throughput: {:.1}M values/sec",
        (n * 4) as f64 / median.as_secs_f64() / 1_000_000.0,
    );

    cleanup(&path);
    Ok(())
}

#[test]
fn bench_read_mixed_100k() -> Result<(), MdfError> {
    let path = temp_path("bench_mixed_100k");
    cleanup(&path);
    let n = 100_000usize;
    write_mixed_file(&path, n)?;

    let iterations = 5;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let mut total = 0usize;
        for group in mdf.channel_groups() {
            for channel in group.channels() {
                total += channel.values()?.len();
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(total, n * 4);
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    eprintln!(
        "bench_read_mixed_100k: median={:.4}s ({} records x 4 mixed channels)",
        median.as_secs_f64(), n,
    );
    eprintln!(
        "  throughput: {:.1}M values/sec",
        (n * 4) as f64 / median.as_secs_f64() / 1_000_000.0,
    );

    cleanup(&path);
    Ok(())
}

/// Benchmark the fast f64 path (values_as_f64)
#[test]
fn bench_read_f64_fast_100k() -> Result<(), MdfError> {
    let path = temp_path("bench_f64_fast_100k");
    cleanup(&path);
    let n = 100_000usize;
    write_f64_file(&path, n)?;

    let _ = MDF::from_file(path.to_str().unwrap())?;

    let iterations = 5;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let mut total = 0usize;
        for group in mdf.channel_groups() {
            for channel in group.channels() {
                total += channel.values_as_f64()?.len();
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(total, n * 4);
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    eprintln!(
        "bench_read_f64_fast_100k (values_as_f64): median={:.4}s ({} records x 4 channels)",
        median.as_secs_f64(), n,
    );
    eprintln!(
        "  throughput: {:.1}M values/sec",
        (n * 4) as f64 / median.as_secs_f64() / 1_000_000.0,
    );

    cleanup(&path);
    Ok(())
}

/// Benchmark the fast f64 path for 1M records
#[test]
fn bench_read_f64_fast_1m() -> Result<(), MdfError> {
    let path = temp_path("bench_f64_fast_1m");
    cleanup(&path);
    let n = 1_000_000usize;
    write_f64_file(&path, n)?;

    let iterations = 3;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let mut total = 0usize;
        for group in mdf.channel_groups() {
            for channel in group.channels() {
                total += channel.values_as_f64()?.len();
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(total, n * 4);
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    eprintln!(
        "bench_read_f64_fast_1m (values_as_f64): median={:.4}s ({} records x 4 channels)",
        median.as_secs_f64(), n,
    );
    eprintln!(
        "  throughput: {:.1}M values/sec",
        (n * 4) as f64 / median.as_secs_f64() / 1_000_000.0,
    );

    cleanup(&path);
    Ok(())
}

/// Benchmark just the file open + metadata parsing (no value decoding)
#[test]
fn bench_open_only_1m() -> Result<(), MdfError> {
    let path = temp_path("bench_open_1m");
    cleanup(&path);
    let n = 1_000_000usize;
    write_f64_file(&path, n)?;

    let iterations = 5;
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let _mdf = MDF::from_file(path.to_str().unwrap())?;
        let elapsed = start.elapsed();
        times.push(elapsed);
    }

    times.sort();
    let median = times[iterations / 2];
    eprintln!(
        "bench_open_only_1m: median={:.6}s (file open + metadata parse)",
        median.as_secs_f64(),
    );

    cleanup(&path);
    Ok(())
}
