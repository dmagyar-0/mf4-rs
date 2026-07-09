//! Regression tests for API-layer fixes: physical values from
//! `values_as_f64`, master-axis conversion in `signal()`, and the
//! all-invalid channel flag.

use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::{BlockHeader, DataType};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("fix_api_{}_{}.mf4", name, std::process::id()))
}

fn linear_cc(a: f64, b: f64) -> ConversionBlock {
    ConversionBlock {
        header: BlockHeader { id: "##CC".to_string(), reserved0: 0, block_len: 0, links_nr: 4 },
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
        cc_phy_range_min: Some(0.0),
        cc_phy_range_max: Some(0.0),
        cc_val: vec![a, b],
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
        default_conversion: None,
    }
}

/// Writes: master Time (f64, linear 0.001 * raw ticks) + Value (f64, linear 10 + 2x).
fn write_file(path: &str, all_invalid_flag: bool) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let tc = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".to_string());
    })?;
    w.set_time_channel(&tc)?;
    let vc = w.add_channel(&cg, Some(&tc), |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Value".to_string());
        if all_invalid_flag {
            c.flags = 0x01; // cn_flags bit 0: all values invalid
        }
    })?;

    // Patch linear conversions onto both channels (CN conversion link @ 56).
    let cc_time = linear_cc(0.0, 0.001).to_bytes()?;
    let cc_val = linear_cc(10.0, 2.0).to_bytes()?;
    let cc_time_pos = w.write_block(&cc_time)?;
    let cc_val_pos = w.write_block(&cc_val)?;
    let tc_pos = w.get_block_position(&tc).unwrap();
    let vc_pos = w.get_block_position(&vc).unwrap();
    w.update_link(tc_pos + 56, cc_time_pos)?;
    w.update_link(vc_pos + 56, cc_val_pos)?;

    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..100u64 {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(i as f64), // raw ticks; physical = i/1000 s
                DecodedValue::Float(i as f64), // physical = 10 + 2i
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()
}

#[test]
fn values_as_f64_applies_conversions() -> Result<(), MdfError> {
    let path = temp("phys");
    write_file(path.to_str().unwrap(), false)?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let ch = mdf.channel("Value").expect("channel");
    let phys = ch.values_as_f64()?;
    assert_eq!(phys.len(), 100);
    for (i, v) in phys.iter().enumerate() {
        let expect = 10.0 + 2.0 * i as f64;
        assert!((v - expect).abs() < 1e-12, "[{}] {} != {}", i, v, expect);
    }

    // values() must agree.
    let vals = ch.values()?;
    match vals[3] {
        Some(DecodedValue::Float(f)) => assert!((f - 16.0).abs() < 1e-12),
        ref other => panic!("unexpected {:?}", other),
    }

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[test]
fn signal_master_axis_is_physical() -> Result<(), MdfError> {
    let path = temp("master");
    write_file(path.to_str().unwrap(), false)?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let sig = mdf.signal("Value")?.expect("signal");
    assert_eq!(sig.timestamps.len(), 100);
    // Master stored as ticks with a 0.001 linear conversion: physical seconds.
    assert!((sig.timestamps[50] - 0.050).abs() < 1e-12, "{}", sig.timestamps[50]);

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[test]
fn all_invalid_flag_without_invalidation_bytes() -> Result<(), MdfError> {
    let path = temp("allinv");
    write_file(path.to_str().unwrap(), true)?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let ch = mdf.channel("Value").expect("channel");

    let vals = ch.values()?;
    assert_eq!(vals.len(), 100);
    assert!(vals.iter().all(|v| v.is_none()), "cn_flags bit 0 must invalidate all samples");

    let f64s = ch.values_as_f64()?;
    assert!(f64s.iter().all(|v| v.is_nan()));

    std::fs::remove_file(&path).ok();
    Ok(())
}
