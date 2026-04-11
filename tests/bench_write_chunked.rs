/// Tests and benchmarks for chunked/streaming columnar writes.
/// Verifies that write_columns_f64 can be called multiple times
/// without requiring the full dataset in memory.
use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::writer::MdfWriter;

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mf4rs_chunked_{}.mf4", name))
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

/// Verify that calling write_columns_f64 multiple times produces
/// the same data as a single call with the full dataset.
#[test]
fn chunked_write_matches_single_write() -> Result<(), MdfError> {
    let n = 10_000usize;

    // Generate full dataset
    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();

    // Write in one shot
    let path_single = temp_path("single");
    cleanup(&path_single);
    {
        let (mut w, cg) = setup_f64_writer(&path_single)?;
        w.write_columns_f64(&cg, &[&col0, &col1, &col2, &col3])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
    }

    // Write in chunks of 1000
    let path_chunked = temp_path("chunked");
    cleanup(&path_chunked);
    {
        let (mut w, cg) = setup_f64_writer(&path_chunked)?;
        let chunk_size = 1000;
        for start in (0..n).step_by(chunk_size) {
            let end = (start + chunk_size).min(n);
            w.write_columns_f64(&cg, &[
                &col0[start..end],
                &col1[start..end],
                &col2[start..end],
                &col3[start..end],
            ])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
    }

    // Read both and compare
    let mdf_single = MDF::from_file(path_single.to_str().unwrap())?;
    let mdf_chunked = MDF::from_file(path_chunked.to_str().unwrap())?;

    let groups_s: Vec<_> = mdf_single.channel_groups();
    let groups_c: Vec<_> = mdf_chunked.channel_groups();

    for (gs, gc) in groups_s.iter().zip(groups_c.iter()) {
        let chs: Vec<_> = gs.channels();
        let chc: Vec<_> = gc.channels();
        assert_eq!(chs.len(), chc.len());
        for (cs, cc) in chs.iter().zip(chc.iter()) {
            let vs = cs.values_as_f64()?;
            let vc = cc.values_as_f64()?;
            assert_eq!(vs.len(), vc.len(), "length mismatch for channel {}", cs.name().unwrap_or(None).unwrap_or_default());
            for i in 0..vs.len() {
                assert!(
                    (vs[i] - vc[i]).abs() < 1e-15,
                    "value mismatch at row {} for channel {}: {} vs {}",
                    i, cs.name().unwrap_or(None).unwrap_or_default(), vs[i], vc[i]
                );
            }
        }
    }

    cleanup(&path_single);
    cleanup(&path_chunked);
    Ok(())
}

/// Verify chunked writes work correctly across DT block boundaries.
/// With 4 x f64 = 32 bytes/record and MAX_DT_BLOCK_SIZE = 4MB,
/// one DT block holds (4*1024*1024 - 24) / 32 = 131071 records.
/// Writing 300K records in 10K chunks should trigger multiple DT splits.
#[test]
fn chunked_write_across_dt_boundaries() -> Result<(), MdfError> {
    let n = 300_000usize;
    let chunk_size = 10_000;

    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();

    let path = temp_path("chunked_dt");
    cleanup(&path);
    {
        let (mut w, cg) = setup_f64_writer(&path)?;
        for start in (0..n).step_by(chunk_size) {
            let end = (start + chunk_size).min(n);
            w.write_columns_f64(&cg, &[
                &col0[start..end],
                &col1[start..end],
                &col2[start..end],
                &col3[start..end],
            ])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
    }

    // Read back and verify
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups: Vec<_> = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let channels: Vec<_> = groups[0].channels();
    assert_eq!(channels.len(), 4);

    let vals0 = channels[0].values_as_f64()?;
    assert_eq!(vals0.len(), n);
    // Check first, last, and boundary values
    assert!((vals0[0] - 0.0).abs() < 1e-15);
    assert!((vals0[n - 1] - (n - 1) as f64 * 0.001).abs() < 1e-10);
    // Check around DT boundary (~131071)
    assert!((vals0[131070] - 131070.0 * 0.001).abs() < 1e-10);
    assert!((vals0[131071] - 131071.0 * 0.001).abs() < 1e-10);

    cleanup(&path);
    Ok(())
}

/// Benchmark: chunked columnar writes vs single-shot,
/// using the same total data (1M records).
#[test]
fn bench_chunked_vs_single_1m() -> Result<(), MdfError> {
    let n = 1_000_000usize;
    let iterations = 3;

    let col0: Vec<f64> = (0..n).map(|i| i as f64 * 0.001).collect();
    let col1: Vec<f64> = col0.iter().map(|v| v * 2.0).collect();
    let col2: Vec<f64> = col0.iter().map(|v| v * 3.0).collect();
    let col3: Vec<f64> = col0.iter().map(|v| v * 4.0).collect();
    let bytes = n * 32;

    // Single-shot
    let mut times_single = Vec::new();
    for _ in 0..iterations {
        let path = temp_path("bench_single_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;
        let start = std::time::Instant::now();
        w.write_columns_f64(&cg, &[&col0, &col1, &col2, &col3])?;
        w.finish_data_block(&cg)?;
        w.finalize()?;
        times_single.push(start.elapsed());
        cleanup(&path);
    }
    times_single.sort();
    let med_single = times_single[iterations / 2];

    // Chunked: 10K records per call
    let mut times_10k = Vec::new();
    for _ in 0..iterations {
        let path = temp_path("bench_chunk10k_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;
        let start = std::time::Instant::now();
        for s in (0..n).step_by(10_000) {
            let e = (s + 10_000).min(n);
            w.write_columns_f64(&cg, &[&col0[s..e], &col1[s..e], &col2[s..e], &col3[s..e]])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
        times_10k.push(start.elapsed());
        cleanup(&path);
    }
    times_10k.sort();
    let med_10k = times_10k[iterations / 2];

    // Chunked: 100K records per call
    let mut times_100k = Vec::new();
    for _ in 0..iterations {
        let path = temp_path("bench_chunk100k_1m");
        cleanup(&path);
        let (mut w, cg) = setup_f64_writer(&path)?;
        let start = std::time::Instant::now();
        for s in (0..n).step_by(100_000) {
            let e = (s + 100_000).min(n);
            w.write_columns_f64(&cg, &[&col0[s..e], &col1[s..e], &col2[s..e], &col3[s..e]])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
        times_100k.push(start.elapsed());
        cleanup(&path);
    }
    times_100k.sort();
    let med_100k = times_100k[iterations / 2];

    eprintln!("bench_chunked_vs_single_1m (4 x f64, 1M records):");
    eprintln!(
        "  single call:      {:.4}s ({:.0} MB/s)",
        med_single.as_secs_f64(),
        bytes as f64 / med_single.as_secs_f64() / 1_048_576.0,
    );
    eprintln!(
        "  10K-row chunks:   {:.4}s ({:.0} MB/s)  overhead: {:.0}%",
        med_10k.as_secs_f64(),
        bytes as f64 / med_10k.as_secs_f64() / 1_048_576.0,
        (med_10k.as_secs_f64() / med_single.as_secs_f64() - 1.0) * 100.0,
    );
    eprintln!(
        "  100K-row chunks:  {:.4}s ({:.0} MB/s)  overhead: {:.0}%",
        med_100k.as_secs_f64(),
        bytes as f64 / med_100k.as_secs_f64() / 1_048_576.0,
        (med_100k.as_secs_f64() / med_single.as_secs_f64() - 1.0) * 100.0,
    );

    Ok(())
}
