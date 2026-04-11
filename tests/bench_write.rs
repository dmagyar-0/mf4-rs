/// Write performance benchmarks for mf4-rs
/// Measures throughput of different write paths and record counts.
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::{MdfWriter, ColumnData};

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mf4rs_wbench_{}.mf4", name))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn setup_f64_writer(path: &std::path::Path) -> Result<(MdfWriter, String), MdfError> {
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
    Ok((w, cg))
}

fn setup_u64_writer(path: &std::path::Path) -> Result<(MdfWriter, String), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&t)?;
    let a = w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("A".into());
        ch.bit_count = 64;
    })?;
    let b = w.add_channel(&cg, Some(&a), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("B".into());
        ch.bit_count = 64;
    })?;
    w.add_channel(&cg, Some(&b), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("C".into());
        ch.bit_count = 64;
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    Ok((w, cg))
}

// ── write_record (single, f64) ──────────────────────────────────────

#[test]
fn bench_write_record_f64_100k() -> Result<(), MdfError> {
    let n = 100_000usize;
    let iterations = 5;
    let mut times = Vec::new();

    for _ in 0..iterations {
        let path = temp_path("wr_f64_100k");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
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
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32; // 4 channels * 8 bytes each
    eprintln!(
        "bench_write_record_f64_100k: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

#[test]
fn bench_write_record_f64_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    for _ in 0..iterations {
        let path = temp_path("wr_f64_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
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
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_record_f64_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── write_records (batch, f64) ──────────────────────────────────────

#[test]
fn bench_write_records_batch_f64_100k() -> Result<(), MdfError> {
    let n = 100_000usize;
    let iterations = 5;
    let mut times = Vec::new();

    // Pre-build records
    let records: Vec<Vec<DecodedValue>> = (0..n)
        .map(|i| {
            let v = i as f64 * 0.001;
            vec![
                DecodedValue::Float(v),
                DecodedValue::Float(v * 2.0),
                DecodedValue::Float(v * 3.0),
                DecodedValue::Float(v * 4.0),
            ]
        })
        .collect();
    let record_refs: Vec<&[DecodedValue]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_batch_f64_100k");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_batch_f64_100k: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

#[test]
fn bench_write_records_batch_f64_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    let records: Vec<Vec<DecodedValue>> = (0..n)
        .map(|i| {
            let v = i as f64 * 0.001;
            vec![
                DecodedValue::Float(v),
                DecodedValue::Float(v * 2.0),
                DecodedValue::Float(v * 3.0),
                DecodedValue::Float(v * 4.0),
            ]
        })
        .collect();
    let record_refs: Vec<&[DecodedValue]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_batch_f64_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_batch_f64_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── write_records_u64 (batch, u64) ──────────────────────────────────

#[test]
fn bench_write_records_u64_100k() -> Result<(), MdfError> {
    let n = 100_000usize;
    let iterations = 5;
    let mut times = Vec::new();

    let records: Vec<Vec<u64>> = (0..n)
        .map(|i| vec![i as u64, i as u64 * 2, i as u64 * 3, i as u64 * 4])
        .collect();
    let record_refs: Vec<&[u64]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_u64_100k");
        cleanup(&path);
        let (mut w, cg) = setup_u64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records_u64(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_u64_100k: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

#[test]
fn bench_write_records_u64_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    let records: Vec<Vec<u64>> = (0..n)
        .map(|i| vec![i as u64, i as u64 * 2, i as u64 * 3, i as u64 * 4])
        .collect();
    let record_refs: Vec<&[u64]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_u64_1m");
        cleanup(&path);
        let (mut w, cg) = setup_u64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records_u64(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_u64_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── write_records_f64 (batch, f64 fast path) ────────────────────────

#[test]
fn bench_write_records_f64_100k() -> Result<(), MdfError> {
    let n = 100_000usize;
    let iterations = 5;
    let mut times = Vec::new();

    let records: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let v = i as f64 * 0.001;
            vec![v, v * 2.0, v * 3.0, v * 4.0]
        })
        .collect();
    let record_refs: Vec<&[f64]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_f64fast_100k");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records_f64(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_f64_100k: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

#[test]
fn bench_write_records_f64_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    let records: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let v = i as f64 * 0.001;
            vec![v, v * 2.0, v * 3.0, v * 4.0]
        })
        .collect();
    let record_refs: Vec<&[f64]> = records.iter().map(|r| r.as_slice()).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_f64fast_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_records_f64(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_records_f64_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── write_columns_f64 (columnar, f64) ───────────────────────────────

#[test]
fn bench_write_columns_f64_100k() -> Result<(), MdfError> {
    let n = 100_000usize;
    let iterations = 5;
    let mut times = Vec::new();

    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_col_f64_100k");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_columns_f64(&cg, &[&col0, &col1, &col2, &col3])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_columns_f64_100k: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

#[test]
fn bench_write_columns_f64_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();

    for _ in 0..iterations {
        let path = temp_path("wr_col_f64_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;

        let start = std::time::Instant::now();
        w.write_columns_f64(&cg, &[&col0, &col1, &col2, &col3])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_columns_f64_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── write_columns (mixed types) ─────────────────────────────────────

#[test]
fn bench_write_columns_mixed_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;
    let mut times = Vec::new();

    let col_u64: Vec<u64> = (0..n).map(|i| i as u64).collect();
    let col_f64: Vec<f64> = (0..n).map(|i| i as f64 * 0.1).collect();
    let col_i64: Vec<i64> = (0..n).map(|i| -(i as i64)).collect();
    let col_u64_2: Vec<u64> = (0..n).map(|i| i as u64 * 1000).collect();

    // Need a writer with mixed types
    for _ in 0..iterations {
        let path = temp_path("wr_col_mixed_1m");
        cleanup(&path);
        let mut w = MdfWriter::new(path.to_str().unwrap())?;
        w.init_mdf_file()?;
        let cg = w.add_channel_group(None, |_| {})?;
        let ch1 = w.add_channel(&cg, None, |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.name = Some("counter".into());
            ch.bit_count = 64;
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

        let start = std::time::Instant::now();
        w.write_columns(&cg, &[
            ColumnData::U64(&col_u64),
            ColumnData::F64(&col_f64),
            ColumnData::I64(&col_i64),
            ColumnData::U64(&col_u64_2),
        ])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        cleanup(&path);
    }

    times.sort();
    let median = times[iterations / 2];
    let bytes = n * 32;
    eprintln!(
        "bench_write_columns_mixed_1m: median={:.4}s ({:.1}M records/s, {:.0} MB/s)",
        median.as_secs_f64(),
        n as f64 / median.as_secs_f64() / 1_000_000.0,
        bytes as f64 / median.as_secs_f64() / 1_048_576.0,
    );
    Ok(())
}

// ── Correctness verification for new write paths ────────────────────

#[test]
fn verify_write_columns_f64_correctness() -> Result<(), MdfError> {
    use mf4_rs::api::mdf::MDF;

    let n = 1000usize;
    let path = temp_path("verify_col_f64");
    cleanup(&path);

    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();

    {
        let (mut w, cg) = setup_f64_writer(&path)?;
        w.write_columns_f64(&cg, &[&col0, &col1, &col2, &col3])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
    }

    // Read back and verify
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups: Vec<_> = mdf.channel_groups().into_iter().collect();
    assert_eq!(groups.len(), 1);
    let channels: Vec<_> = groups[0].channels().into_iter().collect();
    assert_eq!(channels.len(), 4);

    let vals0 = channels[0].values_as_f64()?;
    let vals1 = channels[1].values_as_f64()?;
    let vals2 = channels[2].values_as_f64()?;
    let vals3 = channels[3].values_as_f64()?;

    assert_eq!(vals0.len(), n);
    assert_eq!(vals1.len(), n);

    for i in 0..n {
        assert!((vals0[i] - col0[i]).abs() < 1e-10, "mismatch at row {} ch0", i);
        assert!((vals1[i] - col1[i]).abs() < 1e-10, "mismatch at row {} ch1", i);
        assert!((vals2[i] - col2[i]).abs() < 1e-10, "mismatch at row {} ch2", i);
        assert!((vals3[i] - col3[i]).abs() < 1e-10, "mismatch at row {} ch3", i);
    }

    cleanup(&path);
    Ok(())
}

#[test]
fn verify_write_records_f64_correctness() -> Result<(), MdfError> {
    use mf4_rs::api::mdf::MDF;

    let n = 1000usize;
    let path = temp_path("verify_rec_f64");
    cleanup(&path);

    let records: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let v = i as f64 * 0.001;
            vec![v, v * 2.0, v * 3.0, v * 4.0]
        })
        .collect();
    let record_refs: Vec<&[f64]> = records.iter().map(|r| r.as_slice()).collect();

    {
        let (mut w, cg) = setup_f64_writer(&path)?;
        w.write_records_f64(&cg, record_refs.iter().copied())?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
    }

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups: Vec<_> = mdf.channel_groups().into_iter().collect();
    assert_eq!(groups.len(), 1);
    let channels: Vec<_> = groups[0].channels().into_iter().collect();
    assert_eq!(channels.len(), 4);

    let vals = channels[0].values_as_f64()?;
    assert_eq!(vals.len(), n);
    for i in 0..n {
        let expected = i as f64 * 0.001;
        assert!((vals[i] - expected).abs() < 1e-10, "mismatch at row {}", i);
    }

    cleanup(&path);
    Ok(())
}
