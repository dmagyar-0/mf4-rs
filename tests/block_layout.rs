use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn build_sample(path: &str) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".to_string());
        ch.unit_addr = 0;
    })?;
    writer.set_time_channel(&time_id)?;
    writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Value".to_string());
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    for i in 0..5 {
        writer.write_record(
            &cg_id,
            &[
                DecodedValue::Float(i as f64),
                DecodedValue::Float((i * 10) as f64),
            ],
        )?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;
    Ok(())
}

#[test]
fn layout_covers_core_blocks() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("layout_test.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    build_sample(path.to_str().unwrap())?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let layout = mdf.file_layout()?;

    assert!(layout.file_size > 0);

    let types: Vec<&str> = layout
        .blocks
        .iter()
        .map(|b| b.block_type.as_str())
        .collect();
    for expected in ["##ID", "##HD", "##DG", "##CG", "##CN", "##TX", "##DT"] {
        assert!(
            types.contains(&expected),
            "missing {} in layout: {:?}",
            expected,
            types
        );
    }

    // Every block's end must lie within the file.
    for b in &layout.blocks {
        assert!(
            b.end_offset <= layout.file_size,
            "block {} @ 0x{:x} exceeds file size {}",
            b.block_type,
            b.offset,
            layout.file_size
        );
        assert_eq!(b.end_offset, b.offset + b.size);
    }

    // Blocks must not overlap.
    let mut sorted = layout.blocks.clone();
    sorted.sort_by_key(|b| b.offset);
    for pair in sorted.windows(2) {
        assert!(
            pair[0].end_offset <= pair[1].offset,
            "overlap between {} @ 0x{:x}+{} and {} @ 0x{:x}",
            pair[0].block_type,
            pair[0].offset,
            pair[0].size,
            pair[1].block_type,
            pair[1].offset
        );
    }

    // Header block must link to the first data group, whose offset must be a
    // real block in the layout.
    let hd = layout
        .blocks
        .iter()
        .find(|b| b.block_type == "##HD")
        .expect("HD block");
    let first_dg = hd
        .links
        .iter()
        .find(|l| l.name == "first_dg_addr")
        .expect("first_dg_addr link");
    assert_ne!(first_dg.target, 0);
    assert_eq!(first_dg.target_type.as_deref(), Some("##DG"));
    assert!(layout
        .blocks
        .iter()
        .any(|b| b.offset == first_dg.target && b.block_type == "##DG"));

    // Text blocks for channel names should be discovered.
    let tx_texts: Vec<String> = layout
        .blocks
        .iter()
        .filter(|b| b.block_type == "##TX")
        .map(|b| b.description.clone())
        .collect();
    assert!(tx_texts.iter().any(|d| d.contains("Time")));
    assert!(tx_texts.iter().any(|d| d.contains("Value")));

    // Renderers produce non-empty output.
    let text = layout.to_text();
    assert!(text.contains("##ID"));
    assert!(text.contains("##HD"));
    assert!(text.contains("first_dg_addr"));

    let tree = layout.to_tree();
    assert!(tree.contains("##ID"));
    assert!(tree.contains("##HD"));

    let json = layout.to_json()?;
    let parsed: FileLayout = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.blocks.len(), layout.blocks.len());

    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn layout_from_file_matches_mdf() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("layout_standalone.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    build_sample(path.to_str().unwrap())?;

    let via_mdf = MDF::from_file(path.to_str().unwrap())?.file_layout()?;
    let direct = FileLayout::from_file(path.to_str().unwrap())?;
    assert_eq!(via_mdf.file_size, direct.file_size);
    assert_eq!(via_mdf.blocks.len(), direct.blocks.len());

    std::fs::remove_file(path)?;
    Ok(())
}
