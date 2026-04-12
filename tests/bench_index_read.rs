/// Index-based read performance benchmarks for mf4-rs
/// Measures index read performance across all available paths:
/// - FileRangeReader (original)
/// - MmapRangeReader
/// - Slice-based (zero-copy mmap)
/// - f64 fast paths for each of the above
/// - Direct MDF read for comparison
use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{FileRangeReader, MmapRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mf4rs_bench_idx_{}.mf4", name))
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
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(v),
                DecodedValue::Float(v * 2.0),
                DecodedValue::Float(v * 3.0),
                DecodedValue::Float(v * 4.0),
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(())
}

#[test]
fn bench_index_all_paths_100k() -> Result<(), MdfError> {
    let path = temp_path("idx_all_100k");
    cleanup(&path);
    let n = 100_000usize;
    write_f64_file(&path, n)?;

    let index = MdfIndex::from_file(path.to_str().unwrap())?;
    let iterations = 5;

    // 1. FileRangeReader (DecodedValue)
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut reader = FileRangeReader::new(path.to_str().unwrap())?;
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values(0, ch, &mut reader)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let file_reader_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 2. MmapRangeReader (DecodedValue)
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut reader = MmapRangeReader::new(path.to_str().unwrap())?;
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values(0, ch, &mut reader)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let mmap_reader_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 3. Slice-based zero-copy (DecodedValue)
    let file = std::fs::File::open(path.to_str().unwrap()).unwrap();
    let mmap = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values_from_slice(0, ch, &mmap)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let slice_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 4. FileRangeReader f64 fast path
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut reader = FileRangeReader::new(path.to_str().unwrap())?;
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values_as_f64(0, ch, &mut reader)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let f64_file_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 5. Slice-based f64 fast path (fastest)
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values_from_slice_as_f64(0, ch, &mmap)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let f64_slice_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 6. Direct MDF read (baseline)
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
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let direct_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 7. Direct MDF values_as_f64 (fastest baseline)
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
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let direct_f64_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    let val_count = (n * 4) as f64;
    eprintln!("\n=== Index Read Benchmark: {} records x 4 channels ===", n);
    eprintln!("                          Time(ms)  Throughput(M val/s)");
    eprintln!("  FileRangeReader:        {:7.1}    {:5.1}", file_reader_ms, val_count / file_reader_ms / 1000.0);
    eprintln!("  MmapRangeReader:        {:7.1}    {:5.1}", mmap_reader_ms, val_count / mmap_reader_ms / 1000.0);
    eprintln!("  Slice (zero-copy):      {:7.1}    {:5.1}", slice_ms, val_count / slice_ms / 1000.0);
    eprintln!("  FileReader f64:         {:7.1}    {:5.1}", f64_file_ms, val_count / f64_file_ms / 1000.0);
    eprintln!("  Slice f64 (fastest):    {:7.1}    {:5.1}", f64_slice_ms, val_count / f64_slice_ms / 1000.0);
    eprintln!("  Direct MDF values():    {:7.1}    {:5.1}", direct_ms, val_count / direct_ms / 1000.0);
    eprintln!("  Direct MDF f64:         {:7.1}    {:5.1}", direct_f64_ms, val_count / direct_f64_ms / 1000.0);
    eprintln!("  ---");
    eprintln!("  Speedup slice_f64 vs FileReader: {:.1}x", file_reader_ms / f64_slice_ms);
    eprintln!("  Speedup slice_f64 vs direct_f64: {:.2}x", direct_f64_ms / f64_slice_ms);

    cleanup(&path);
    Ok(())
}

#[test]
fn bench_index_all_paths_1m() -> Result<(), MdfError> {
    let path = temp_path("idx_all_1m");
    cleanup(&path);
    let n = 1_000_000usize;
    write_f64_file(&path, n)?;

    let index = MdfIndex::from_file(path.to_str().unwrap())?;
    let iterations = 3;

    // 1. FileRangeReader (DecodedValue)
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut reader = FileRangeReader::new(path.to_str().unwrap())?;
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values(0, ch, &mut reader)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let file_reader_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 2. Slice f64 (fastest)
    let file = std::fs::File::open(path.to_str().unwrap()).unwrap();
    let mmap = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let mut total = 0usize;
        for ch in 0..4 {
            total += index.read_channel_values_from_slice_as_f64(0, ch, &mmap)?.len();
        }
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let f64_slice_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    // 3. Direct MDF f64 (baseline)
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
        assert_eq!(total, n * 4);
        times.push(start.elapsed());
    }
    times.sort();
    let direct_f64_ms = times[iterations / 2].as_secs_f64() * 1000.0;

    let val_count = (n * 4) as f64;
    eprintln!("\n=== Index Read Benchmark: {} records x 4 channels ===", n);
    eprintln!("                          Time(ms)  Throughput(M val/s)");
    eprintln!("  FileRangeReader:        {:7.1}    {:5.1}", file_reader_ms, val_count / file_reader_ms / 1000.0);
    eprintln!("  Slice f64 (fastest):    {:7.1}    {:5.1}", f64_slice_ms, val_count / f64_slice_ms / 1000.0);
    eprintln!("  Direct MDF f64:         {:7.1}    {:5.1}", direct_f64_ms, val_count / direct_f64_ms / 1000.0);
    eprintln!("  ---");
    eprintln!("  Speedup slice_f64 vs FileReader: {:.1}x", file_reader_ms / f64_slice_ms);
    eprintln!("  Speedup slice_f64 vs direct_f64: {:.2}x", direct_f64_ms / f64_slice_ms);

    cleanup(&path);
    Ok(())
}

/// Verify correctness: all paths produce identical results
#[test]
fn test_index_read_paths_consistency() -> Result<(), MdfError> {
    let path = temp_path("idx_consistency");
    cleanup(&path);
    let n = 1000usize;
    write_f64_file(&path, n)?;

    let index = MdfIndex::from_file(path.to_str().unwrap())?;

    // Read via all paths
    let mut file_reader = FileRangeReader::new(path.to_str().unwrap())?;
    let mut mmap_reader = MmapRangeReader::new(path.to_str().unwrap())?;
    let file = std::fs::File::open(path.to_str().unwrap()).unwrap();
    let mmap = unsafe { memmap2::Mmap::map(&file) }.unwrap();

    for ch in 0..4 {
        let vals_file = index.read_channel_values(0, ch, &mut file_reader)?;
        let vals_mmap = index.read_channel_values(0, ch, &mut mmap_reader)?;
        let vals_slice = index.read_channel_values_from_slice(0, ch, &mmap)?;

        let f64_file = index.read_channel_values_as_f64(0, ch, &mut file_reader)?;
        let f64_slice = index.read_channel_values_from_slice_as_f64(0, ch, &mmap)?;

        assert_eq!(vals_file.len(), n);
        assert_eq!(vals_mmap.len(), n);
        assert_eq!(vals_slice.len(), n);
        assert_eq!(f64_file.len(), n);
        assert_eq!(f64_slice.len(), n);

        // All DecodedValue paths should be identical
        for i in 0..n {
            assert_eq!(vals_file[i], vals_mmap[i], "mmap mismatch at ch={} i={}", ch, i);
            assert_eq!(vals_file[i], vals_slice[i], "slice mismatch at ch={} i={}", ch, i);
        }

        // f64 paths should match
        for i in 0..n {
            assert!(
                (f64_file[i] - f64_slice[i]).abs() < 1e-15,
                "f64 mismatch at ch={} i={}: {} vs {}", ch, i, f64_file[i], f64_slice[i]
            );
        }

        // f64 should match DecodedValue float values
        for i in 0..n {
            if let Some(DecodedValue::Float(expected)) = &vals_file[i] {
                assert!(
                    (f64_file[i] - expected).abs() < 1e-15,
                    "f64 vs decoded mismatch at ch={} i={}: {} vs {}", ch, i, f64_file[i], expected
                );
            }
        }
    }

    cleanup(&path);
    Ok(())
}
