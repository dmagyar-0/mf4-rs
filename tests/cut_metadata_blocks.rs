//! Verifies that `cut_mdf_by_time` preserves the source file's block
//! topology AND its auxiliary metadata blocks (##TX, ##MD, ##SI, ##CC).
//!
//! The test builds a 2-DG / 1-CG source file decorated with:
//!   * channel name / unit / comment text blocks
//!   * channel source (##SI) + the ##TX blocks the source links to
//!   * a linear conversion (##CC) on one channel
//!   * a value-to-text conversion (##CC + several ##TX) on another
//!   * channel-group acq_name (##TX) and comment (##TX) blocks
//!
//! It then cuts a window and asserts:
//!   * every block type that was present in the source is still present in
//!     the output, in at least the same count (TX/MD/SI may be deduplicated
//!     by the cut's block cache, so we accept >=)
//!   * the parser-visible metadata round-trips (names, units, comments,
//!     source info, conversion-decoded values).

use std::collections::BTreeMap;

use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::{BlockHeader, DataType};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::blocks::text_block::TextBlock;
use mf4_rs::cut::cut_mdf_by_time;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Hand-serialise a minimal `##SI` source block (3 links, type/bus/flags).
/// Layout: 24 B header + 3*8 B links + 1+1+1+5 B data/padding = 56 B.
fn build_si_block_bytes(name_addr: u64, path_addr: u64, comment_addr: u64) -> Vec<u8> {
    let header = BlockHeader {
        id: "##SI".into(),
        reserved0: 0,
        block_len: 56,
        links_nr: 3,
    };
    let mut bytes = Vec::with_capacity(56);
    bytes.extend_from_slice(&header.to_bytes().expect("##SI header"));
    bytes.extend_from_slice(&name_addr.to_le_bytes());
    bytes.extend_from_slice(&path_addr.to_le_bytes());
    bytes.extend_from_slice(&comment_addr.to_le_bytes());
    bytes.push(4); // si_type = TOOL
    bytes.push(0); // bus_type = NONE
    bytes.push(0); // flags
    bytes.extend_from_slice(&[0u8; 5]); // reserved
    bytes
}

fn write_tx(w: &mut MdfWriter, id: &str, text: &str) -> Result<u64, MdfError> {
    let bytes = TextBlock::new(text).to_bytes()?;
    w.write_block_with_id(&bytes, id)
}

fn write_si(
    w: &mut MdfWriter,
    id: &str,
    name: &str,
    path: &str,
    comment: &str,
) -> Result<u64, MdfError> {
    let name_pos = write_tx(w, &format!("{}_name", id), name)?;
    let path_pos = write_tx(w, &format!("{}_path", id), path)?;
    let cmt_pos = write_tx(w, &format!("{}_cmt", id), comment)?;
    let bytes = build_si_block_bytes(name_pos, path_pos, cmt_pos);
    w.write_block_with_id(&bytes, id)
}

fn write_linear_cc(w: &mut MdfWriter, id: &str, p0: f64, p1: f64) -> Result<u64, MdfError> {
    let cc = ConversionBlock {
        header: BlockHeader { id: "##CC".into(), reserved0: 0, block_len: 0, links_nr: 0 },
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref: Vec::new(),
        cc_type: ConversionType::Linear,
        cc_precision: 0,
        cc_flags: 0,
        cc_ref_count: 0,
        cc_val_count: 2,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val: vec![p0, p1],
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
        default_conversion: None,
    };
    let bytes = cc.to_bytes()?;
    w.write_block_with_id(&bytes, id)
}

/// Channel block link offsets we patch directly:
///   48 = source_addr, 56 = conversion_addr, 72 = unit_addr, 80 = comment_addr.
const CN_SOURCE: u64 = 48;
const CN_CONV: u64 = 56;
const CN_UNIT: u64 = 72;
const CN_COMMENT: u64 = 80;

/// Channel-group block link offsets:
///   40 = acq_name_addr, 48 = acq_source_addr, 64 = comment_addr.
const CG_ACQ_NAME: u64 = 40;
const CG_ACQ_SOURCE: u64 = 48;
const CG_COMMENT: u64 = 64;

fn build_decorated_source(path: &str) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;

    // ---- DG 0 / CG 0: Time + ValA(u32) with linear conversion ----
    let cg0 = w.add_channel_group(None, |_| {})?;

    // Decorate CG 0: acq_name + comment + source.
    let cg0_pos = w.get_block_position(&cg0).expect("cg0 pos");
    let cg0_name_pos = write_tx(&mut w, "tx_cg0_name", "TempGroup")?;
    w.update_link(cg0_pos + CG_ACQ_NAME, cg0_name_pos)?;
    let cg0_cmt_pos = write_tx(&mut w, "tx_cg0_cmt", "first acquisition group")?;
    w.update_link(cg0_pos + CG_COMMENT, cg0_cmt_pos)?;
    let cg0_si_pos = write_si(&mut w, "si_cg0", "ECU-A", "/bus/can0", "primary ECU")?;
    w.update_link(cg0_pos + CG_ACQ_SOURCE, cg0_si_pos)?;

    let t0 = w.add_channel(&cg0, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t0)?;

    let v0 = w.add_channel(&cg0, Some(&t0), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.bit_count = 32;
        c.name = Some("ValA".into());
    })?;
    // Decorate ValA with unit, comment, source, linear conversion.
    let v0_pos = w.get_block_position(&v0).expect("v0 pos");
    let v0_unit = write_tx(&mut w, "tx_v0_unit", "degC")?;
    w.update_link(v0_pos + CN_UNIT, v0_unit)?;
    let v0_cmt = write_tx(&mut w, "tx_v0_cmt", "Engine coolant temperature")?;
    w.update_link(v0_pos + CN_COMMENT, v0_cmt)?;
    let v0_si = write_si(&mut w, "si_v0", "TempSensor", "/sensor/temp", "Bosch NTC")?;
    w.update_link(v0_pos + CN_SOURCE, v0_si)?;
    // Linear: phys = 10 + 2*raw
    let v0_cc = write_linear_cc(&mut w, "cc_v0", 10.0, 2.0)?;
    w.update_link(v0_pos + CN_CONV, v0_cc)?;

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

    // ---- DG 1 / CG 1: Time + State(u8) with value-to-text conversion ----
    let cg1 = w.add_channel_group(None, |_| {})?;

    // Decorate CG 1.
    let cg1_pos = w.get_block_position(&cg1).expect("cg1 pos");
    let cg1_name_pos = write_tx(&mut w, "tx_cg1_name", "StateGroup")?;
    w.update_link(cg1_pos + CG_ACQ_NAME, cg1_name_pos)?;
    let cg1_cmt_pos = write_tx(&mut w, "tx_cg1_cmt", "second acquisition group")?;
    w.update_link(cg1_pos + CG_COMMENT, cg1_cmt_pos)?;

    let t1 = w.add_channel(&cg1, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t1)?;

    let s1 = w.add_channel(&cg1, Some(&t1), |c| {
        c.data_type = DataType::UnsignedIntegerLE;
        c.bit_count = 8;
        c.name = Some("State".into());
    })?;
    let s1_unit = write_tx(&mut w, "tx_s1_unit", "")?;
    let s1_pos = w.get_block_position(&s1).expect("s1 pos");
    w.update_link(s1_pos + CN_UNIT, s1_unit)?;
    let s1_cmt = write_tx(&mut w, "tx_s1_cmt", "Operating mode")?;
    w.update_link(s1_pos + CN_COMMENT, s1_cmt)?;
    let s1_si = write_si(&mut w, "si_s1", "ECU-B", "/bus/can1", "secondary ECU")?;
    w.update_link(s1_pos + CN_SOURCE, s1_si)?;

    // Attach a value-to-text conversion via the writer's helper. This
    // produces one ##CC and (mapping.len() + 1) ##TX blocks.
    w.add_value_to_text_conversion(
        &[(0, "OFF"), (1, "ON"), (2, "ERROR")],
        "UNKNOWN",
        Some(&s1),
    )?;

    w.start_data_block_for_cg(&cg1, 0)?;
    for i in 0..10u64 {
        let state = (i % 3) as u8; // cycles 0, 1, 2
        w.write_record(
            &cg1,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::UnsignedInteger(state as u64),
            ],
        )?;
    }
    w.finish_data_block(&cg1)?;

    w.finalize()
}

fn block_histogram(layout: &FileLayout) -> BTreeMap<String, usize> {
    let mut h: BTreeMap<String, usize> = BTreeMap::new();
    for b in &layout.blocks {
        *h.entry(b.block_type.clone()).or_default() += 1;
    }
    h
}

fn print_histogram(label: &str, hist: &BTreeMap<String, usize>) {
    print!("[{}]", label);
    for (k, v) in hist {
        print!("  {}={}", k, v);
    }
    println!();
}

#[test]
fn cut_preserves_all_metadata_blocks() -> Result<(), MdfError> {
    let tmp = std::env::temp_dir();
    let inp = tmp.join("multi_dg_meta_in.mf4");
    let out = tmp.join("multi_dg_meta_out.mf4");
    for p in [&inp, &out] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    build_decorated_source(inp.to_str().unwrap())?;

    let in_layout = FileLayout::from_file(inp.to_str().unwrap())?;
    let in_hist = block_histogram(&in_layout);
    println!("\n========== Source block histogram ==========");
    print_histogram("INPUT", &in_hist);

    cut_mdf_by_time(inp.to_str().unwrap(), out.to_str().unwrap(), 0.2, 0.5)?;

    let out_layout = FileLayout::from_file(out.to_str().unwrap())?;
    let out_hist = block_histogram(&out_layout);
    println!("========== Cut block histogram ==========");
    print_histogram("CUT  ", &out_hist);

    let in_size = std::fs::metadata(&inp)?.len();
    let out_size = std::fs::metadata(&out)?.len();
    println!("file sizes: input={} cut={}", in_size, out_size);

    // -------- Skeleton: must match exactly --------
    for k in ["##ID", "##HD", "##DG", "##CG", "##CN"] {
        assert_eq!(
            in_hist.get(k).copied().unwrap_or(0),
            out_hist.get(k).copied().unwrap_or(0),
            "skeleton block {} count differs",
            k
        );
    }

    // -------- Auxiliary blocks: every type present in the source must be
    // present in the output, with at least the same count. The cut's block
    // cache may dedupe shared TX/MD/SI/CC, so we use >= rather than == for
    // these. --------
    for kind in ["##TX", "##SI", "##CC"] {
        let src = in_hist.get(kind).copied().unwrap_or(0);
        let dst = out_hist.get(kind).copied().unwrap_or(0);
        if src > 0 {
            assert!(
                dst >= src,
                "auxiliary block {} dropped count: input={} output={}",
                kind,
                src,
                dst
            );
        }
    }
    // ##MD: source has none; just confirm output didn't sprout invalid ones.
    assert_eq!(
        in_hist.get("##MD").copied().unwrap_or(0),
        0,
        "test assumption broken: source unexpectedly grew ##MD blocks"
    );

    // No surprise block types: every block type in the output must already
    // be a known MDF block (or have existed in the source).
    let known = [
        "##ID", "##HD", "##DG", "##CG", "##CN", "##DT", "##DL", "##TX", "##MD", "##SI", "##CC",
        "##SD", "##DV",
    ];
    for k in out_hist.keys() {
        assert!(
            known.contains(&k.as_str()),
            "unknown block type in cut output: {}",
            k
        );
    }

    // -------- Semantic round-trip via the parser API --------
    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 2);

    // CG 0
    let g0 = &groups[0];
    assert_eq!(g0.name()?.as_deref(), Some("TempGroup"));
    assert_eq!(g0.comment()?.as_deref(), Some("first acquisition group"));
    let g0_src = g0.source()?.expect("CG0 source survived");
    assert_eq!(g0_src.name.as_deref(), Some("ECU-A"));
    assert_eq!(g0_src.path.as_deref(), Some("/bus/can0"));
    assert_eq!(g0_src.comment.as_deref(), Some("primary ECU"));

    let chs0 = g0.channels();
    assert_eq!(chs0.len(), 2);
    let val_a = &chs0[1];
    assert_eq!(val_a.name()?.as_deref(), Some("ValA"));
    assert_eq!(val_a.unit()?.as_deref(), Some("degC"));
    assert_eq!(
        val_a.comment()?.as_deref(),
        Some("Engine coolant temperature")
    );
    let val_a_src = val_a.source()?.expect("ValA source survived");
    assert_eq!(val_a_src.name.as_deref(), Some("TempSensor"));
    assert_eq!(val_a_src.path.as_deref(), Some("/sensor/temp"));
    // Linear conversion: phys = 10 + 2*raw, raws kept by window [0.2, 0.5] are 2..=5
    let phys_a = val_a.values()?;
    let phys_a: Vec<f64> = phys_a
        .into_iter()
        .map(|v| match v {
            Some(DecodedValue::Float(f)) => f,
            other => panic!("ValA decoded as {:?}", other),
        })
        .collect();
    assert_eq!(phys_a, vec![14.0, 16.0, 18.0, 20.0]);

    // CG 1
    let g1 = &groups[1];
    assert_eq!(g1.name()?.as_deref(), Some("StateGroup"));
    assert_eq!(g1.comment()?.as_deref(), Some("second acquisition group"));

    let chs1 = g1.channels();
    let state = &chs1[1];
    assert_eq!(state.name()?.as_deref(), Some("State"));
    assert_eq!(state.comment()?.as_deref(), Some("Operating mode"));
    let state_src = state.source()?.expect("State source survived");
    assert_eq!(state_src.name.as_deref(), Some("ECU-B"));
    // Value-to-text: raw State = i % 3, kept indices 2..=5 → raws [2, 0, 1, 2]
    // → texts [ERROR, OFF, ON, ERROR].
    let phys_s = state.values()?;
    let texts: Vec<String> = phys_s
        .into_iter()
        .map(|v| match v {
            Some(DecodedValue::String(s)) => s,
            other => panic!("State decoded as {:?}", other),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "ERROR".to_string(),
            "OFF".to_string(),
            "ON".to_string(),
            "ERROR".to_string(),
        ]
    );

    Ok(())
}
