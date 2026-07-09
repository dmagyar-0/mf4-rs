// Regression tests for the writer fixes (FINDINGS A7, A10, A12, D1, D3, D4,
// D5, D6, C8): hard errors instead of silent zero-encoding, invalidation-byte
// framing, DL equal-length semantics, write_record_u64 auto-splitting,
// degenerate splits, VLSD channel normalization and header start time.

use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::common::{BlockParse, DataType};
use mf4_rs::blocks::data_group_block::DataGroupBlock;
use mf4_rs::blocks::data_list_block::DataListBlock;
use mf4_rs::blocks::header_block::HeaderBlock;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    if p.exists() {
        std::fs::remove_file(&p).unwrap();
    }
    p
}

fn read_u64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
}

/// Follow ID block -> HD -> first DG and return (dg_addr, data_addr,
/// first_cg_addr) from the raw file bytes.
fn first_dg(bytes: &[u8]) -> (u64, u64, u64) {
    let hd = HeaderBlock::from_bytes(&bytes[64..]).unwrap();
    let dg_addr = hd.first_dg_addr;
    let dg = DataGroupBlock::from_bytes(&bytes[dg_addr as usize..]).unwrap();
    (dg_addr, dg.data_block_addr, dg.first_cg_addr)
}

// ---------------------------------------------------------------------------
// A7: unsupported data types must error instead of silently writing zeros
// ---------------------------------------------------------------------------

#[test]
fn fixed_string_channel_errors_on_start() -> Result<(), MdfError> {
    let path = tmp("fixw_fixed_string.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::StringUtf8; // fixed-length string, NOT VLSD
        ch.bit_count = 64;
        ch.name = Some("Label".into());
    })?;
    let err = w.start_data_block_for_cg(&cg, 0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Label"), "error should name the channel: {msg}");
    assert!(msg.contains("StringUtf8"), "error should name the data type: {msg}");
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
fn big_endian_channel_errors_on_start() -> Result<(), MdfError> {
    let path = tmp("fixw_be.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerBE;
        ch.bit_count = 32;
        ch.name = Some("BigEnd".into());
    })?;
    assert!(w.start_data_block_for_cg(&cg, 0).is_err());
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
fn type_mismatched_value_errors_on_write() -> Result<(), MdfError> {
    let path = tmp("fixw_mismatch.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 32;
        ch.name = Some("U32".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;

    // Float into an unsigned integer channel: hard error.
    assert!(w.write_record(&cg, &[DecodedValue::Float(1.5)]).is_err());
    // String into a non-VLSD channel: hard error.
    assert!(w
        .write_record(&cg, &[DecodedValue::String("x".into())])
        .is_err());
    // Negative signed value into an unsigned channel: hard error.
    assert!(w
        .write_record(&cg, &[DecodedValue::SignedInteger(-1)])
        .is_err());
    // Non-negative signed value into an unsigned channel: accepted.
    w.write_record(&cg, &[DecodedValue::SignedInteger(7)])?;
    // Plain unsigned value: accepted.
    w.write_record(&cg, &[DecodedValue::UnsignedInteger(9)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let values = mdf.channel_groups()[0].channels()[0].values()?;
    assert_eq!(
        values,
        vec![
            Some(DecodedValue::UnsignedInteger(7)),
            Some(DecodedValue::UnsignedInteger(9))
        ]
    );
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
fn unsigned_out_of_range_into_signed_errors() -> Result<(), MdfError> {
    let path = tmp("fixw_signed_range.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::SignedIntegerLE;
        ch.bit_count = 64;
        ch.name = Some("I64".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    assert!(w
        .write_record(&cg, &[DecodedValue::UnsignedInteger(u64::MAX)])
        .is_err());
    w.write_record(&cg, &[DecodedValue::UnsignedInteger(42)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// A10: bit_offset != 0 is unsupported by the encoders and must error
// ---------------------------------------------------------------------------

#[test]
fn nonzero_bit_offset_errors_on_start() -> Result<(), MdfError> {
    let path = tmp("fixw_bit_offset.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 4;
        ch.bit_offset = 3;
        ch.name = Some("Nibble".into());
    })?;
    let err = w.start_data_block_for_cg(&cg, 0).unwrap_err();
    assert!(format!("{err}").contains("bit_offset"));
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D3: FloatLE bit_count must be 32 or 64
// ---------------------------------------------------------------------------

#[test]
fn f16_float_errors_on_start() -> Result<(), MdfError> {
    let path = tmp("fixw_f16.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 16; // half float: unsupported
        ch.name = Some("Half".into());
    })?;
    let err = w.start_data_block_for_cg(&cg, 0).unwrap_err();
    assert!(format!("{err}").contains("32 or 64"));
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// A12: invalidation_bytes_nr set on the channel group must be part of the
// written record stride
// ---------------------------------------------------------------------------

#[test]
fn invalidation_bytes_round_trip_with_correct_stride() -> Result<(), MdfError> {
    let path = tmp("fixw_inval.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |cg| {
        cg.invalidation_bytes_nr = 2;
    })?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 32;
        ch.name = Some("Value".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    let n = 10u64;
    for i in 0..n {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::UnsignedInteger(i * 100),
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    // The declared stride must match the written stride: DT data section
    // length == cycles * (samples_byte_nr + invalidation_bytes_nr).
    let bytes = std::fs::read(&path)?;
    let (_, data_addr, cg_addr) = first_dg(&bytes);
    let cgb = ChannelGroupBlock::from_bytes(&bytes[cg_addr as usize..]).unwrap();
    assert_eq!(cgb.samples_byte_nr, 12, "8 (f64) + 4 (u32) data bytes");
    assert_eq!(cgb.invalidation_bytes_nr, 2);
    assert_eq!(cgb.cycles_nr, n);
    let dt_len = read_u64(&bytes, data_addr as usize + 8);
    assert_eq!(
        dt_len - 24,
        n * (cgb.samples_byte_nr + cgb.invalidation_bytes_nr) as u64,
        "DT data section must include the invalidation bytes"
    );

    // The file must decode correctly (invalidation bytes are zero-filled, so
    // every value is valid).
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    let channels = groups[0].channels();
    let vals = channels[1].values()?;
    assert_eq!(vals.len(), n as usize);
    for (i, v) in vals.iter().enumerate() {
        assert_eq!(*v, Some(DecodedValue::UnsignedInteger(i as u64 * 100)));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D1: dl_equal_length must be the fragment DATA length (block_len - 24)
// ---------------------------------------------------------------------------

#[test]
fn dl_equal_length_is_fragment_data_length() -> Result<(), MdfError> {
    let path = tmp("fixw_dl_equal.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    // 64 KiB byte-array records force a DT split after 63 records.
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 64 * 1024 * 8;
        ch.name = Some("Blob".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..100u8 {
        w.write_record(&cg, &[DecodedValue::ByteArray(vec![i; 16])])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let bytes = std::fs::read(&path)?;
    let (_, data_addr, _) = first_dg(&bytes);
    // The DG data link must now point at a ##DL block.
    assert_eq!(&bytes[data_addr as usize..data_addr as usize + 4], b"##DL");
    let dl = DataListBlock::from_bytes(&bytes[data_addr as usize..]).unwrap();
    assert_eq!(dl.flags & 1, 1, "equal-length DL expected");
    assert!(dl.data_links.len() >= 2, "expected at least two fragments");

    // Every fragment must be non-empty, and dl_equal_length must equal the
    // FIRST fragment's data-section length (block_len - 24), not its full
    // block length.
    let first_dt_len = read_u64(&bytes, dl.data_links[0] as usize + 8);
    assert!(first_dt_len > 24);
    assert_eq!(dl.data_block_len, Some(first_dt_len - 24));
    for &link in &dl.data_links {
        assert_eq!(&bytes[link as usize..link as usize + 4], b"##DT");
        assert!(read_u64(&bytes, link as usize + 8) > 24, "empty DT fragment");
    }

    // Round trip still works across the fragment chain.
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    let vals = groups[0].channels()[0].values()?;
    assert_eq!(vals.len(), 100);
    match &vals[99] {
        Some(DecodedValue::ByteArray(b)) => {
            assert_eq!(b.len(), 64 * 1024);
            assert_eq!(b[0], 99);
        }
        other => panic!("expected ByteArray, got {:?}", other),
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D4: write_record_u64 must auto-split at MAX_DT_BLOCK_SIZE like every other
// write path
// ---------------------------------------------------------------------------

#[test]
fn write_record_u64_splits_at_4mb() -> Result<(), MdfError> {
    let path = tmp("fixw_u64_split.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let mut prev: Option<String> = None;
    for i in 0..8 {
        let id = w.add_channel(&cg, prev.as_deref(), |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.bit_count = 64;
            ch.name = Some(format!("U{}", i));
        })?;
        prev = Some(id);
    }
    w.start_data_block_for_cg(&cg, 0)?;
    // 8 channels x 8 bytes = 64-byte records; 70_000 records = ~4.5 MB > 4 MB.
    let n = 70_000u64;
    for i in 0..n {
        let rec = [i, i + 1, i + 2, i + 3, i + 4, i + 5, i + 6, i + 7];
        w.write_record_u64(&cg, &rec)?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let bytes = std::fs::read(&path)?;
    let (_, data_addr, _) = first_dg(&bytes);
    assert_eq!(
        &bytes[data_addr as usize..data_addr as usize + 4],
        b"##DL",
        "write_record_u64 must split oversized DT blocks into a DL chain"
    );
    let dl = DataListBlock::from_bytes(&bytes[data_addr as usize..]).unwrap();
    assert!(dl.data_links.len() >= 2);

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    let channels = groups[0].channels();
    let vals = channels[0].values()?;
    assert_eq!(vals.len(), n as usize);
    assert_eq!(vals[0], Some(DecodedValue::UnsignedInteger(0)));
    assert_eq!(vals[n as usize - 1], Some(DecodedValue::UnsignedInteger(n - 1)));
    let vals7 = channels[7].values()?;
    assert_eq!(vals7[n as usize - 1], Some(DecodedValue::UnsignedInteger(n - 1 + 7)));
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Degenerate split: a record larger than the 4 MB threshold must not create
// an empty first fragment
// ---------------------------------------------------------------------------

#[test]
fn oversized_record_does_not_create_empty_fragment() -> Result<(), MdfError> {
    let path = tmp("fixw_oversized.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    // 5 MB record: larger than MAX_DT_BLOCK_SIZE - 24.
    let record_bytes: usize = 5 * 1024 * 1024;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = (record_bytes * 8) as u32;
        ch.name = Some("Huge".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[DecodedValue::ByteArray(vec![1; 8])])?;
    w.write_record(&cg, &[DecodedValue::ByteArray(vec![2; 8])])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let bytes = std::fs::read(&path)?;
    let (_, data_addr, _) = first_dg(&bytes);
    assert_eq!(&bytes[data_addr as usize..data_addr as usize + 4], b"##DL");
    let dl = DataListBlock::from_bytes(&bytes[data_addr as usize..]).unwrap();
    assert_eq!(dl.data_links.len(), 2, "one fragment per oversized record");
    for &link in &dl.data_links {
        let len = read_u64(&bytes, link as usize + 8);
        assert_eq!(
            len as usize,
            24 + record_bytes,
            "each fragment holds exactly one record — never zero"
        );
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D6: VLSD channels are normalized on add_channel (bit_count = 64, no bogus
// data link on disk) and detected by channel_type == 1 alone
// ---------------------------------------------------------------------------

#[test]
fn vlsd_channel_gets_bit_count_64_and_null_data_link() -> Result<(), MdfError> {
    use mf4_rs::blocks::channel_block::ChannelBlock;

    let path = tmp("fixw_vlsd_norm.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::StringUtf8;
        ch.channel_type = 1; // VLSD — note: no `ch.data = 1` sentinel needed
        ch.bit_count = 32; // wrong on purpose; must be forced to 64
        ch.name = Some("Msg".into());
    })?;
    // Finalize WITHOUT ever opening a data block: the CN.data link must be 0
    // (not the in-memory placeholder), so readers do not follow a bogus
    // address.
    w.finalize()?;

    let bytes = std::fs::read(&path)?;
    let (_, _, cg_addr) = first_dg(&bytes);
    let mut cgb = ChannelGroupBlock::from_bytes(&bytes[cg_addr as usize..]).unwrap();
    let channels = cgb.read_channels(&bytes).unwrap();
    let vlsd: Vec<&ChannelBlock> =
        channels.iter().filter(|c| c.channel_type == 1).collect();
    assert_eq!(vlsd.len(), 1);
    assert_eq!(vlsd[0].bit_count, 64, "VLSD slot is a u64 offset");
    assert_eq!(vlsd[0].data, 0, "no placeholder data link on disk");
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
fn vlsd_write_read_round_trip_without_sentinel() -> Result<(), MdfError> {
    let path = tmp("fixw_vlsd_rt.mf4");
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::StringUtf8;
        ch.channel_type = 1; // detection must work WITHOUT ch.data = 1
        ch.name = Some("Msg".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    let msgs = ["alpha", "bravo", "charlie"];
    for (i, m) in msgs.iter().enumerate() {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(i as f64 * 0.5),
                DecodedValue::String((*m).into()),
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    let channels = groups[0].channels();
    let vals = channels[1].values()?;
    assert_eq!(vals.len(), msgs.len());
    for (v, want) in vals.iter().zip(msgs.iter()) {
        match v {
            Some(DecodedValue::String(s)) => assert_eq!(s, want),
            other => panic!("expected String, got {:?}", other),
        }
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D5: header abs_time defaults to 0, and init_mdf_file stamps the current
// system time
// ---------------------------------------------------------------------------

#[test]
fn header_default_abs_time_is_zero() {
    let hd = HeaderBlock::default();
    assert_eq!(hd.abs_time, 0, "no more undocumented 2-hour magic value");
}

#[test]
fn init_mdf_file_sets_current_start_time() -> Result<(), MdfError> {
    let path = tmp("fixw_abs_time.mf4");
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    w.finalize()?;
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let start = mdf
        .start_time_ns()
        .expect("writer must stamp a non-zero start time");
    assert!(
        start >= before && start <= after,
        "start_time_ns {start} not within [{before}, {after}]"
    );

    // set_start_time must still be able to override it.
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    w.set_start_time(1_700_000_000_000_000_000, 60, 0, 0b11, 0)?;
    w.finalize()?;
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    assert_eq!(mdf.start_time_ns(), Some(1_700_000_000_000_000_000));
    std::fs::remove_file(&path)?;
    Ok(())
}
