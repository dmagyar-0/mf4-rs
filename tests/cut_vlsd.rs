use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Build an MDF file containing a VLSD ("signal-based") byte channel using
/// the lower-level writer APIs, then cut a sub-window and verify the kept
/// VLSD payloads round-trip exactly.
#[test]
fn cut_preserves_vlsd_byte_channel() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_vlsd_input.mf4");
    let output = std::env::temp_dir().join("cut_vlsd_output.mf4");
    if input.exists() {
        std::fs::remove_file(&input)?;
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }

    // Build source: time (f64 master) + VLSD ByteArray channel. The parent
    // record reserves 8 bytes for the VLSD slot (we write zeros there; the
    // library iterates SD entries sequentially regardless of slot value).
    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    let vlsd_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 64;
        ch.channel_type = 1; // VLSD
        ch.name = Some("Payload".into());
    })?;

    // Open the parent DT block and the VLSD ##SD chain, then write 10
    // records: time + 8 zero bytes (VLSD slot) and one variable-length
    // payload per record.
    writer.start_data_block_for_cg_raw(
        &cg_id,
        /* record_id_len */ 0,
        /* data_bytes */ 16, // 8 (time) + 8 (vlsd slot)
        /* invalidation_bytes */ 0,
    )?;
    writer.start_signal_data_block(&vlsd_id)?;

    let payloads: Vec<Vec<u8>> = (0..10u64)
        .map(|i| format!("event-{}-payload", i).into_bytes())
        .collect();

    for i in 0..10u64 {
        let mut record = Vec::with_capacity(16);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&[0u8; 8]); // VLSD inline slot
        writer.write_raw_record(&cg_id, &record)?;
        writer.write_signal_data(&vlsd_id, &payloads[i as usize])?;
    }
    writer.finish_signal_data_block(&vlsd_id)?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Sanity check: the source file's VLSD channel reads back correctly.
    {
        let mdf = MDF::from_file(input.to_str().unwrap())?;
        let groups = mdf.channel_groups();
        let chs = groups[0].channels();
        let read_payloads = chs[1].values()?;
        assert_eq!(read_payloads.len(), 10);
        for (i, v) in read_payloads.iter().enumerate() {
            match v {
                Some(DecodedValue::ByteArray(b)) => assert_eq!(b, &payloads[i]),
                other => panic!("source[{}]: expected ByteArray, got {:?}", i, other),
            }
        }
    }

    // Cut [0.2, 0.6] — should keep records 2..=6 with their VLSD entries.
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.2,
        0.6,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let chs = groups[0].channels();
    assert_eq!(chs.len(), 2);

    let times = chs[0].values()?;
    let read_payloads = chs[1].values()?;
    assert_eq!(times.len(), 5, "expected 5 kept records");
    assert_eq!(read_payloads.len(), 5);

    let expected: Vec<Vec<u8>> = (2u64..=6u64)
        .map(|i| format!("event-{}-payload", i).into_bytes())
        .collect();

    for (i, v) in read_payloads.iter().enumerate() {
        match v {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b, &expected[i]),
            other => panic!("cut[{}]: expected ByteArray, got {:?}", i, other),
        }
    }
    if let Some(DecodedValue::Float(t)) = times[0] {
        assert!((t - 0.2).abs() < 1e-9, "first kept time = {}", t);
    } else {
        panic!("unexpected time[0]: {:?}", times[0]);
    }
    if let Some(DecodedValue::Float(t)) = times[4] {
        assert!((t - 0.6).abs() < 1e-9, "last kept time = {}", t);
    } else {
        panic!("unexpected time[4]: {:?}", times[4]);
    }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}

/// Per the MDF4 spec, the parent record of a VLSD channel carries the byte
/// offset of the entry within the linked SD block's data section. mf4-rs's own
/// reader walks SD entries sequentially so it ignores that offset, but
/// spec-conformant readers (e.g. asammdf) use it for lookup. Cutting must
/// rewrite each kept record's inline slot to point at the entry's location in
/// the freshly written ##SD block — otherwise the cut output is unreadable by
/// asammdf even though it still round-trips through mf4-rs.
///
/// This test simulates an asammdf-style file by populating non-zero inline
/// offsets at write time, cuts a window, and asserts that the inline offsets
/// in the cut output match the new SD layout (i.e. start at 0 and advance by
/// `4 + payload.len()` per kept entry).
#[test]
fn cut_rewrites_vlsd_inline_offsets() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_vlsd_offsets_input.mf4");
    let output = std::env::temp_dir().join("cut_vlsd_offsets_output.mf4");
    if input.exists() {
        std::fs::remove_file(&input)?;
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }

    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    let vlsd_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::StringUtf8;
        ch.bit_count = 64;
        ch.channel_type = 1; // VLSD
        ch.name = Some("Message".into());
    })?;

    writer.start_data_block_for_cg_raw(&cg_id, 0, 16, 0)?;
    writer.start_signal_data_block(&vlsd_id)?;

    // Variable-length payloads so SD entry sizes differ — that's what makes
    // the offset rewrite non-trivial.
    let payloads: Vec<Vec<u8>> = (0..8u64)
        .map(|i| format!("msg-{}-{}", i, "x".repeat(i as usize)).into_bytes())
        .collect();

    let mut running: u64 = 0;
    for (i, payload) in payloads.iter().enumerate() {
        let mut record = Vec::with_capacity(16);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        // Inline VLSD slot = byte offset within SD data section. This
        // mimics asammdf's writer.
        record.extend_from_slice(&running.to_le_bytes());
        writer.write_raw_record(&cg_id, &record)?;
        writer.write_signal_data(&vlsd_id, payload)?;
        running = running.checked_add(4 + payload.len() as u64).unwrap();
    }
    writer.finish_signal_data_block(&vlsd_id)?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Cut [0.2, 0.5] -> keep records 2..=5.
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.2,
        0.5,
    )?;

    // Sanity: mf4-rs still reads the cut file correctly.
    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let read_payloads = chs[1].values()?;
    assert_eq!(read_payloads.len(), 4);
    let expected: Vec<Vec<u8>> = (2u64..=5u64)
        .map(|i| format!("msg-{}-{}", i, "x".repeat(i as usize)).into_bytes())
        .collect();
    for (i, v) in read_payloads.iter().enumerate() {
        match v {
            Some(DecodedValue::String(s)) => assert_eq!(s.as_bytes(), &expected[i][..]),
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b, &expected[i]),
            other => panic!("cut[{}]: unexpected value {:?}", i, other),
        }
    }
    drop(mdf);

    // Inspect the raw bytes of the output to assert that each kept record's
    // inline VLSD slot points at the entry's offset in the new SD block.
    // The slot is at bytes [8..16] of every 16-byte parent record.
    let bytes = std::fs::read(&output)?;
    fn find_block(bytes: &[u8], id: &[u8; 4]) -> Option<usize> {
        let mut off = 64usize;
        while off + 24 <= bytes.len() {
            if &bytes[off..off + 4] == id {
                return Some(off);
            }
            off += 8;
        }
        None
    }
    let dt_off = find_block(&bytes, b"##DT").expect("output should contain a ##DT block");
    let dt_len = u64::from_le_bytes(bytes[dt_off + 8..dt_off + 16].try_into().unwrap()) as usize;
    let dt_data = &bytes[dt_off + 24..dt_off + dt_len];
    assert_eq!(
        dt_data.len() % 16,
        0,
        "cut DT block should contain whole 16-byte records"
    );
    assert_eq!(dt_data.len() / 16, 4, "expected 4 kept records");

    let mut expected_offset: u64 = 0;
    for (i, rec) in dt_data.chunks_exact(16).enumerate() {
        let slot = u64::from_le_bytes(rec[8..16].try_into().unwrap());
        assert_eq!(
            slot, expected_offset,
            "kept record {}: inline VLSD slot = 0x{:x}, expected 0x{:x}",
            i, slot, expected_offset
        );
        expected_offset = expected_offset
            .checked_add(4 + expected[i].len() as u64)
            .unwrap();
    }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}

/// Cut should also handle VLSD channels carrying empty payloads correctly.
#[test]
fn cut_preserves_empty_vlsd_payloads() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_vlsd_empty_input.mf4");
    let output = std::env::temp_dir().join("cut_vlsd_empty_output.mf4");
    if input.exists() {
        std::fs::remove_file(&input)?;
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }

    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    let vlsd_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 64;
        ch.channel_type = 1;
        ch.name = Some("MaybeEmpty".into());
    })?;
    writer.start_data_block_for_cg_raw(&cg_id, 0, 16, 0)?;
    writer.start_signal_data_block(&vlsd_id)?;

    // Record indices 1, 3 carry empty payloads; the rest carry data.
    let payloads: Vec<Vec<u8>> = (0..6u64)
        .map(|i| if i == 1 || i == 3 { Vec::new() } else { vec![i as u8; (i + 1) as usize] })
        .collect();
    for i in 0..6u64 {
        let mut record = Vec::with_capacity(16);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&[0u8; 8]);
        writer.write_raw_record(&cg_id, &record)?;
        writer.write_signal_data(&vlsd_id, &payloads[i as usize])?;
    }
    writer.finish_signal_data_block(&vlsd_id)?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Cut [0.1, 0.4] — kept indices 1..=4 = empty, full, empty, full.
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.1,
        0.4,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let read_payloads = chs[1].values()?;
    assert_eq!(read_payloads.len(), 4);
    let expected = [
        payloads[1].clone(),
        payloads[2].clone(),
        payloads[3].clone(),
        payloads[4].clone(),
    ];
    for (i, v) in read_payloads.iter().enumerate() {
        match v {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b, &expected[i]),
            other => panic!("cut[{}]: expected ByteArray, got {:?}", i, other),
        }
    }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}
