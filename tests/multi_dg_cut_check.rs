//! Verifies that `cut_mdf_by_time` preserves the source file's data-group /
//! channel-group block topology when the source has multiple data groups.
//!
//! Builds a 2-DG / 1-CG-each source file, cuts a sub-window, and checks:
//!   * the output has the same number of `##DG`, `##CG`, and `##CN` blocks
//!   * each output `##DG` carries exactly one `##CG` (same as the source)
//!   * the cut window's records read back correctly from both groups
//!   * the file is smaller than the source (less data, same skeleton)
//!
//! Note: the high-level writer API (`add_channel_group`) always creates a
//! fresh `##DG` per channel group, so we can't easily author a 1-DG / N-CG
//! source file from this crate. `cut.rs` mirrors that behaviour: every
//! source CG is materialised into its own DG in the output, so a hand-built
//! shared-DG source would NOT round-trip through cut with the same
//! topology — see the README of this test for details.

use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::DataType;
use mf4_rs::cut::cut_mdf_by_time;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn count_blocks(layout: &FileLayout, kind: &str) -> usize {
    layout.blocks.iter().filter(|b| b.block_type == kind).count()
}

fn dg_first_cg_targets(layout: &FileLayout) -> Vec<u64> {
    layout
        .blocks
        .iter()
        .filter(|b| b.block_type == "##DG")
        .map(|b| {
            b.links
                .iter()
                .find(|l| l.name == "first_cg_addr")
                .map(|l| l.target)
                .unwrap_or(0)
        })
        .collect()
}

fn cg_next_targets(layout: &FileLayout) -> Vec<u64> {
    layout
        .blocks
        .iter()
        .filter(|b| b.block_type == "##CG")
        .map(|b| {
            b.links
                .iter()
                .find(|l| l.name == "next_cg_addr")
                .map(|l| l.target)
                .unwrap_or(0)
        })
        .collect()
}

fn report(label: &str, path: &str) -> Result<(usize, usize, usize, usize, u64), MdfError> {
    let layout = FileLayout::from_file(path)?;
    let dg = count_blocks(&layout, "##DG");
    let cg = count_blocks(&layout, "##CG");
    let cn = count_blocks(&layout, "##CN");
    let dt = count_blocks(&layout, "##DT");
    let dl = count_blocks(&layout, "##DL");
    let size = std::fs::metadata(path)?.len();
    println!(
        "[{}] {} bytes  ##DG={} ##CG={} ##CN={} ##DT={} ##DL={}",
        label, size, dg, cg, cn, dt, dl
    );
    println!("  DG.first_cg targets:  {:?}", dg_first_cg_targets(&layout));
    println!("  CG.next_cg  targets:  {:?}", cg_next_targets(&layout));
    let mdf = MDF::from_file(path)?;
    for (i, g) in mdf.channel_groups().iter().enumerate() {
        let chs = g.channels();
        let n = chs.first().and_then(|c| c.values().ok()).map(|v| v.len()).unwrap_or(0);
        println!("    cg[{}] channels={} records={}", i, chs.len(), n);
    }
    Ok((dg, cg, cn, dt, size))
}

fn build_two_dg_one_cg(path: &str) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;

    // DG 0 / CG 0: Time + ValA (u32)
    let cg0 = w.add_channel_group(None, |_| {})?;
    let t0 = w.add_channel(&cg0, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t0)?;
    w.add_channel(&cg0, Some(&t0), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.bit_count = 32;
        c.name = Some("ValA".into());
    })?;
    w.start_data_block_for_cg(&cg0, 0)?;
    for i in 0..10u64 {
        w.write_record(
            &cg0,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::UnsignedInteger(i),
            ],
        )?;
    }
    w.finish_data_block(&cg0)?;

    // DG 1 / CG 1: Time + ValB (f64)
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
    for i in 0..10u64 {
        w.write_record(
            &cg1,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::Float(100.0 + i as f64),
            ],
        )?;
    }
    w.finish_data_block(&cg1)?;

    w.finalize()
}

#[test]
fn cut_preserves_multi_dg_topology() -> Result<(), MdfError> {
    let tmp = std::env::temp_dir();
    let inp = tmp.join("multi_dg_cut_in.mf4");
    let out = tmp.join("multi_dg_cut_out.mf4");
    for p in [&inp, &out] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    println!("\n========== 2 DG x 1 CG  ==========");
    build_two_dg_one_cg(inp.to_str().unwrap())?;
    let (in_dg, in_cg, in_cn, in_dt, in_size) = report("INPUT", inp.to_str().unwrap())?;

    cut_mdf_by_time(inp.to_str().unwrap(), out.to_str().unwrap(), 0.2, 0.5)?;
    let (out_dg, out_cg, out_cn, out_dt, out_size) = report("CUT  ", out.to_str().unwrap())?;

    // Block skeleton must be identical.
    assert_eq!(in_dg, out_dg, "DG count differs");
    assert_eq!(in_cg, out_cg, "CG count differs");
    assert_eq!(in_cn, out_cn, "CN count differs");
    assert_eq!(in_dt, out_dt, "DT count differs");
    // File must shrink (less data, same skeleton).
    assert!(out_size < in_size, "cut file ({}) not smaller than input ({})", out_size, in_size);

    // Each output DG still has exactly one CG (no re-shaping).
    let layout = FileLayout::from_file(out.to_str().unwrap())?;
    for tgt in dg_first_cg_targets(&layout) {
        assert!(tgt != 0, "an output ##DG has no first_cg link");
    }
    for tgt in cg_next_targets(&layout) {
        assert_eq!(tgt, 0, "an output ##CG unexpectedly chains to another CG");
    }

    // Records survived the cut from both groups.
    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 2);
    for g in &groups {
        let chs = g.channels();
        assert_eq!(chs.len(), 2);
        let times = chs[0].values()?;
        assert_eq!(times.len(), 4, "expected 4 records in window [0.2, 0.5]");
        if let Some(DecodedValue::Float(t0)) = times[0] {
            assert!((t0 - 0.2).abs() < 1e-6);
        }
        if let Some(DecodedValue::Float(t_last)) = times[3] {
            assert!((t_last - 0.5).abs() < 1e-6);
        }
    }

    Ok(())
}
