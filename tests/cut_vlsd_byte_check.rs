//! Diagnostic: when `cut_mdf_by_time` runs across a multi-DG file where one
//! DG carries a VLSD channel and another DG carries only fixed-length
//! channels, are the bytes of each kept record byte-identical between the
//! source and the cut output?
//!
//! Expectation based on `src/cut.rs`:
//!   * VLSD parent records: bytes 0..vlsd_slot_off match the source, the
//!     inline VLSD slot is rewritten to point at the freshly written ##SD
//!     block, bytes after the slot match the source.
//!   * Non-VLSD records: bytes match exactly.
//!   * VLSD ##SD payloads: bytes per kept entry match exactly.
//!
//! The test prints a summary so the diagnostic is visible with `cargo test
//! -- --nocapture`, and also asserts these invariants.

use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::DataType;
use mf4_rs::cut::cut_mdf_by_time;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::parsing::mdf_file::MdfFile;
use mf4_rs::writer::MdfWriter;

const VLSD_SLOT_OFF: usize = 8;
const VLSD_SLOT_LEN: usize = 8;
const RECORD_LEN_VLSD: usize = 16; // 8 bytes time + 8 bytes VLSD slot
const RECORD_LEN_PLAIN: usize = 16; // 8 bytes time + 8 bytes f64 value
const N_RECORDS: u64 = 10;

fn build_source(path: &str) -> Result<Vec<Vec<u8>>, MdfError> {
    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;

    // -- DG 0 / CG 0: Time (master) + VLSD ByteArray "Payload" --
    let cg0 = w.add_channel_group(None, |_| {})?;
    let t0 = w.add_channel(&cg0, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t0)?;
    let vlsd_id = w.add_channel(&cg0, Some(&t0), |c| {
        c.data_type = DataType::ByteArray;
        c.bit_count = 64;
        c.channel_type = 1; // VLSD
        c.name = Some("Payload".into());
    })?;
    w.start_data_block_for_cg_raw(&cg0, 0, RECORD_LEN_VLSD as u32, 0)?;
    w.start_signal_data_block(&vlsd_id)?;

    let payloads: Vec<Vec<u8>> = (0..N_RECORDS)
        .map(|i| format!("event-{}-x{}", i, "y".repeat(i as usize)).into_bytes())
        .collect();

    // Pre-fill non-zero inline slots so we can see them get rewritten.
    let mut running: u64 = 0;
    for i in 0..N_RECORDS {
        let mut record = Vec::with_capacity(RECORD_LEN_VLSD);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&running.to_le_bytes());
        w.write_raw_record(&cg0, &record)?;
        w.write_signal_data(&vlsd_id, &payloads[i as usize])?;
        running = running.checked_add(4 + payloads[i as usize].len() as u64).unwrap();
    }
    w.finish_signal_data_block(&vlsd_id)?;
    w.finish_data_block(&cg0)?;

    // -- DG 1 / CG 1: Time (master) + ValB (f64) --
    let cg1 = w.add_channel_group(None, |_| {})?;
    let t1 = w.add_channel(&cg1, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t1)?;
    w.add_channel(&cg1, Some(&t1), |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("ValB".into());
    })?;
    w.start_data_block_for_cg(&cg1, 0)?;
    for i in 0..N_RECORDS {
        w.write_record(
            &cg1,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::Float(100.0 + i as f64),
            ],
        )?;
    }
    w.finish_data_block(&cg1)?;

    w.finalize()?;
    Ok(payloads)
}

/// Collect all parent records for a given DG by concatenating the data
/// fragments referenced by its `##DT`/`##DL` chain.
fn collect_records(mdf: &MdfFile, dg_idx: usize, record_size: usize) -> Vec<Vec<u8>> {
    let dg = &mdf.data_groups[dg_idx];
    let blocks = dg.data_blocks(&mdf.mmap).expect("data_blocks");
    let mut out = Vec::new();
    for db in blocks {
        for chunk in db.data.chunks_exact(record_size) {
            out.push(chunk.to_vec());
        }
    }
    out
}

#[test]
fn cut_vlsd_byte_level_diff_across_multi_dg() -> Result<(), MdfError> {
    let tmp = std::env::temp_dir();
    let inp = tmp.join("cut_vlsd_byte_check_in.mf4");
    let out = tmp.join("cut_vlsd_byte_check_out.mf4");
    for p in [&inp, &out] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    let src_payloads = build_source(inp.to_str().unwrap())?;

    // Cut [0.2, 0.6] -> keep indices 2..=6 from each DG.
    cut_mdf_by_time(inp.to_str().unwrap(), out.to_str().unwrap(), 0.2, 0.6)?;

    // Block topology must survive (sanity).
    let in_layout = FileLayout::from_file(inp.to_str().unwrap())?;
    let out_layout = FileLayout::from_file(out.to_str().unwrap())?;
    let count = |l: &FileLayout, k: &str| l.blocks.iter().filter(|b| b.block_type == k).count();
    println!(
        "[INPUT ] ##DG={} ##CG={} ##CN={} ##DT={} ##SD={} ##DL={}",
        count(&in_layout, "##DG"),
        count(&in_layout, "##CG"),
        count(&in_layout, "##CN"),
        count(&in_layout, "##DT"),
        count(&in_layout, "##SD"),
        count(&in_layout, "##DL"),
    );
    println!(
        "[CUT   ] ##DG={} ##CG={} ##CN={} ##DT={} ##SD={} ##DL={}",
        count(&out_layout, "##DG"),
        count(&out_layout, "##CG"),
        count(&out_layout, "##CN"),
        count(&out_layout, "##DT"),
        count(&out_layout, "##SD"),
        count(&out_layout, "##DL"),
    );
    assert_eq!(count(&in_layout, "##DG"), count(&out_layout, "##DG"));
    assert_eq!(count(&in_layout, "##CG"), count(&out_layout, "##CG"));
    assert_eq!(count(&in_layout, "##CN"), count(&out_layout, "##CN"));

    let src = MdfFile::parse_from_file(inp.to_str().unwrap())?;
    let dst = MdfFile::parse_from_file(out.to_str().unwrap())?;

    // ---------- DG 0: VLSD channel ----------
    let src_records_vlsd = collect_records(&src, 0, RECORD_LEN_VLSD);
    let dst_records_vlsd = collect_records(&dst, 0, RECORD_LEN_VLSD);
    assert_eq!(src_records_vlsd.len(), N_RECORDS as usize);
    assert_eq!(dst_records_vlsd.len(), 5, "expected 5 kept records in [0.2, 0.6]");

    println!("\n--- DG 0 (VLSD): byte-level record comparison ---");
    let kept_indices: Vec<usize> = (2..=6).collect();
    let mut total_byte_diffs = 0usize;
    let mut byte_diffs_outside_slot = 0usize;
    for (out_i, src_i) in kept_indices.iter().enumerate() {
        let s = &src_records_vlsd[*src_i];
        let d = &dst_records_vlsd[out_i];
        assert_eq!(s.len(), d.len());
        let mut diff_positions = Vec::new();
        for (j, (a, b)) in s.iter().zip(d.iter()).enumerate() {
            if a != b {
                diff_positions.push(j);
                total_byte_diffs += 1;
                let in_slot = j >= VLSD_SLOT_OFF && j < VLSD_SLOT_OFF + VLSD_SLOT_LEN;
                if !in_slot {
                    byte_diffs_outside_slot += 1;
                }
            }
        }
        let src_slot = u64::from_le_bytes(s[VLSD_SLOT_OFF..VLSD_SLOT_OFF + VLSD_SLOT_LEN].try_into().unwrap());
        let dst_slot = u64::from_le_bytes(d[VLSD_SLOT_OFF..VLSD_SLOT_OFF + VLSD_SLOT_LEN].try_into().unwrap());
        println!(
            "  src_idx={} out_idx={}  diffs_at={:?}  src_slot=0x{:x}  dst_slot=0x{:x}",
            src_i, out_i, diff_positions, src_slot, dst_slot,
        );
    }
    println!(
        "  -> total differing bytes across kept VLSD records: {} (outside the inline slot: {})",
        total_byte_diffs, byte_diffs_outside_slot,
    );
    assert_eq!(
        byte_diffs_outside_slot, 0,
        "VLSD records should only differ in the inline slot bytes"
    );
    assert!(
        total_byte_diffs > 0,
        "VLSD inline slots should have been rewritten by cut"
    );

    // VLSD payload (SD entry) bytes must match exactly for kept records.
    println!("\n--- DG 0 (VLSD): SD payload comparison ---");
    let dst_groups = MDF::from_file(out.to_str().unwrap())?;
    let dst_payload_chs = dst_groups.channel_groups()[0].channels();
    let dst_payloads_decoded = dst_payload_chs[1].values()?;
    assert_eq!(dst_payloads_decoded.len(), 5);
    for (out_i, src_i) in kept_indices.iter().enumerate() {
        match &dst_payloads_decoded[out_i] {
            Some(DecodedValue::ByteArray(b)) => {
                assert_eq!(b, &src_payloads[*src_i], "SD payload bytes differ at out_idx={}", out_i);
                println!("  out_idx={} payload_len={} bytes_match=true", out_i, b.len());
            }
            other => panic!("unexpected payload value at out_idx={}: {:?}", out_i, other),
        }
    }

    // ---------- DG 1: non-VLSD ----------
    let src_records_plain = collect_records(&src, 1, RECORD_LEN_PLAIN);
    let dst_records_plain = collect_records(&dst, 1, RECORD_LEN_PLAIN);
    assert_eq!(src_records_plain.len(), N_RECORDS as usize);
    assert_eq!(dst_records_plain.len(), 5);

    println!("\n--- DG 1 (non-VLSD): byte-level record comparison ---");
    let mut plain_diffs = 0usize;
    for (out_i, src_i) in kept_indices.iter().enumerate() {
        let s = &src_records_plain[*src_i];
        let d = &dst_records_plain[out_i];
        let identical = s == d;
        if !identical {
            plain_diffs += 1;
        }
        println!("  src_idx={} out_idx={}  identical={}", src_i, out_i, identical);
    }
    assert_eq!(
        plain_diffs, 0,
        "non-VLSD records should be byte-identical between source and cut output"
    );

    println!("\nSummary:");
    println!("  - VLSD records: bytes outside the 8-byte inline slot are byte-identical;");
    println!("    the inline slot is rewritten to reference the freshly written ##SD block.");
    println!("  - VLSD SD payloads (the actual variable-length data): byte-identical.");
    println!("  - Non-VLSD records (other DGs): byte-identical.");

    std::fs::remove_file(inp)?;
    std::fs::remove_file(out)?;
    Ok(())
}
