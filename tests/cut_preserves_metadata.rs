//! End-to-end checks that `cut_mdf_by_time` preserves the per-channel
//! source/text/conversion blocks from the input file.

use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::{BlockHeader, DataType};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::blocks::text_block::TextBlock;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn cleanup(path: &std::path::Path) {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

/// Write a simple SI block with name/path/comment, return its block id.
fn write_source_block(
    writer: &mut MdfWriter,
    id: &str,
    name: &str,
    path: &str,
    comment: &str,
) -> Result<(), MdfError> {
    // Emit referenced TX blocks first.
    let name_id = format!("{}_name", id);
    let path_id = format!("{}_path", id);
    let comment_id = format!("{}_comment", id);
    writer.write_block_with_id(&TextBlock::new(name).to_bytes()?, &name_id)?;
    writer.write_block_with_id(&TextBlock::new(path).to_bytes()?, &path_id)?;
    writer.write_block_with_id(&TextBlock::new(comment).to_bytes()?, &comment_id)?;

    // Hand-build the SI block: 24-byte header + 3*8-byte links + 8 bytes of
    // data/padding = 56 bytes total.
    let header = BlockHeader {
        id: "##SI".into(),
        reserved0: 0,
        block_len: 56,
        links_nr: 3,
    };
    let mut bytes = Vec::with_capacity(56);
    bytes.extend_from_slice(&header.to_bytes()?);
    bytes.extend_from_slice(&0u64.to_le_bytes()); // name_addr — patched below
    bytes.extend_from_slice(&0u64.to_le_bytes()); // path_addr — patched below
    bytes.extend_from_slice(&0u64.to_le_bytes()); // comment_addr — patched below
    bytes.push(4); // source_type = TOOL
    bytes.push(0); // bus_type = NONE
    bytes.push(0); // flags
    bytes.extend_from_slice(&[0u8; 5]); // reserved/padding
    writer.write_block_with_id(&bytes, id)?;

    // Patch links: name at offset 24, path 32, comment 40.
    writer.update_block_link(id, 24, &name_id)?;
    writer.update_block_link(id, 32, &path_id)?;
    writer.update_block_link(id, 40, &comment_id)?;

    Ok(())
}

#[test]
fn cut_preserves_conversion_unit_comment_and_source() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_meta_input.mf4");
    let output = std::env::temp_dir().join("cut_meta_output.mf4");
    cleanup(&input);
    cleanup(&output);

    // 1. Build a source file with a time channel and a value channel that
    //    has a linear conversion, a unit, a comment, and an SI source block.
    {
        let mut writer = MdfWriter::new(input.to_str().unwrap())?;
        writer.init_mdf_file()?;
        let cg_id = writer.add_channel_group(None, |_| {})?;

        let time_id = writer.add_channel(&cg_id, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.bit_count = 64;
            ch.name = Some("Time".into());
        })?;
        writer.set_time_channel(&time_id)?;

        let val_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.bit_count = 32;
            ch.name = Some("Val".into());
        })?;

        // Conversion: phys = 1 + 3 * raw → raw=2 → phys=7, raw=3 → phys=10, etc.
        let conv = ConversionBlock {
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
            cc_val: vec![1.0, 3.0],
            formula: None,
            resolved_texts: None,
            resolved_conversions: None,
            default_conversion: None,
        };
        let cc_id = "cc_lin".to_string();
        writer.write_block_with_id(&conv.to_bytes()?, &cc_id)?;
        // Patch val channel's conversion link (offset 56 in ##CN block).
        writer.update_block_link(&val_id, 56, &cc_id)?;

        // Unit text block (link offset 72 in ##CN).
        let unit_id = "tx_unit".to_string();
        writer.write_block_with_id(&TextBlock::new("kPa").to_bytes()?, &unit_id)?;
        writer.update_block_link(&val_id, 72, &unit_id)?;

        // Comment text block (link offset 80 in ##CN).
        let comment_id = "tx_comment".to_string();
        writer.write_block_with_id(
            &TextBlock::new("pressure sensor").to_bytes()?,
            &comment_id,
        )?;
        writer.update_block_link(&val_id, 80, &comment_id)?;

        // Source info block (link offset 48 in ##CN).
        write_source_block(&mut writer, "si_val", "ECU-A", "/path/to/ecu", "ecu comment")?;
        writer.update_block_link(&val_id, 48, "si_val")?;

        writer.start_data_block_for_cg(&cg_id, 0)?;
        for i in 0..10u64 {
            writer.write_record(
                &cg_id,
                &[
                    DecodedValue::Float(i as f64 * 0.1),
                    DecodedValue::UnsignedInteger(i),
                ],
            )?;
        }
        writer.finish_data_block(&cg_id)?;
        writer.finalize()?;
    }

    // 2. Sanity check: the source file reads back the metadata as expected.
    {
        let mdf = MDF::from_file(input.to_str().unwrap())?;
        let chs = mdf.channel_groups()[0].channels();
        let val = &chs[1];
        assert_eq!(val.unit()?.as_deref(), Some("kPa"));
        assert_eq!(val.comment()?.as_deref(), Some("pressure sensor"));
        let src = val.source()?.expect("source info");
        assert_eq!(src.name.as_deref(), Some("ECU-A"));
        assert_eq!(src.path.as_deref(), Some("/path/to/ecu"));
        assert_eq!(src.comment.as_deref(), Some("ecu comment"));

        // Conversion applied: raw=3 → phys=10.
        let vals = val.values()?;
        if let Some(DecodedValue::Float(v)) = vals[3] {
            assert!((v - 10.0).abs() < 1e-9, "source phys[3] = {}", v);
        } else {
            panic!("unexpected source vals[3]: {:?}", vals[3]);
        }
    }

    // 3. Cut a slice and verify all of the per-channel metadata survives.
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.2,
        0.5,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let chs = groups[0].channels();
    assert_eq!(chs.len(), 2);
    let val = &chs[1];

    // Unit, comment, and source info round-tripped.
    assert_eq!(val.unit()?.as_deref(), Some("kPa"), "unit was lost");
    assert_eq!(
        val.comment()?.as_deref(),
        Some("pressure sensor"),
        "comment was lost"
    );
    let src = val.source()?.expect("source info missing after cut");
    assert_eq!(src.name.as_deref(), Some("ECU-A"));
    assert_eq!(src.path.as_deref(), Some("/path/to/ecu"));
    assert_eq!(src.comment.as_deref(), Some("ecu comment"));

    // Conversion still applied: raws [2,3,4,5] → phys [7,10,13,16].
    let vals = val.values()?;
    let phys: Vec<f64> = vals
        .iter()
        .map(|v| match v {
            Some(DecodedValue::Float(f)) => *f,
            other => panic!("unexpected cut val: {:?}", other),
        })
        .collect();
    assert_eq!(
        phys,
        vec![7.0, 10.0, 13.0, 16.0],
        "conversion was not preserved or was double-applied"
    );

    cleanup(&input);
    cleanup(&output);
    Ok(())
}

#[test]
fn cut_preserves_chained_value_to_text_conversion() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_v2t_input.mf4");
    let output = std::env::temp_dir().join("cut_v2t_output.mf4");
    cleanup(&input);
    cleanup(&output);

    // Source file with a value-to-text conversion (which references multiple
    // ##TX blocks via cc_ref). cut should clone all of those.
    {
        let mut writer = MdfWriter::new(input.to_str().unwrap())?;
        writer.init_mdf_file()?;
        let cg_id = writer.add_channel_group(None, |_| {})?;
        let time_id = writer.add_channel(&cg_id, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.bit_count = 64;
            ch.name = Some("Time".into());
        })?;
        writer.set_time_channel(&time_id)?;
        let val_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.bit_count = 32;
            ch.name = Some("State".into());
        })?;
        writer.add_value_to_text_conversion(
            &[(0, "OFF"), (1, "ON"), (2, "FAULT")],
            "UNKNOWN",
            Some(&val_id),
        )?;

        writer.start_data_block_for_cg(&cg_id, 0)?;
        for (i, code) in (0u64..6).zip([0u64, 1, 2, 1, 0, 99]) {
            writer.write_record(
                &cg_id,
                &[
                    DecodedValue::Float(i as f64 * 0.1),
                    DecodedValue::UnsignedInteger(code),
                ],
            )?;
        }
        writer.finish_data_block(&cg_id)?;
        writer.finalize()?;
    }

    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.0,
        1.0,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let vals = chs[1].values()?;
    let texts: Vec<String> = vals
        .iter()
        .map(|v| match v {
            Some(DecodedValue::String(s)) => s.clone(),
            other => panic!("expected string, got {:?}", other),
        })
        .collect();
    assert_eq!(
        texts,
        vec!["OFF", "ON", "FAULT", "ON", "OFF", "UNKNOWN"],
        "value-to-text conversion was not preserved across cut"
    );

    cleanup(&input);
    cleanup(&output);
    Ok(())
}
