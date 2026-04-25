use mf4_rs::writer::MdfWriter;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::merge::merge_files;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;

#[test]
fn merge_simple_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_1.mf4");
    let f2 = dir.join("mf4_merge_2.mf4");
    let out = dir.join("mf4_merge_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    // first file with one record value 1
    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    w1.write_record(&cg1, &[DecodedValue::UnsignedInteger(1)])?;
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    // second file with one record value 2
    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    w2.write_record(&cg2, &[DecodedValue::UnsignedInteger(2)])?;
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    assert_eq!(values.len(), 2);

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_float_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_f1.mf4");
    let f2 = dir.join("mf4_merge_f2.mf4");
    let out = dir.join("mf4_merge_f_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let v1 = [0.1f64, 0.2, 0.3];
    let v2 = [0.4f64, 0.5];

    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::FloatLE; ch.bit_count = 64; })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    for v in &v1 { w1.write_record(&cg1, &[DecodedValue::Float(*v)])?; }
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::FloatLE; ch.bit_count = 64; })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    for v in &v2 { w2.write_record(&cg2, &[DecodedValue::Float(*v)])?; }
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1, "matching float groups must concatenate");
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    let expected: Vec<f64> = v1.iter().chain(v2.iter()).copied().collect();
    assert_eq!(values.len(), expected.len());
    for (got, want) in values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::Float(g)) => assert_eq!(g.to_bits(), want.to_bits()),
            other => panic!("expected Float, got {:?}", other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_signed_int_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_si1.mf4");
    let f2 = dir.join("mf4_merge_si2.mf4");
    let out = dir.join("mf4_merge_si_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let v1: [i64; 2] = [-2, -1];
    let v2: [i64; 3] = [0, 1, 2];

    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::SignedIntegerLE; ch.bit_count = 32; })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    for v in &v1 { w1.write_record(&cg1, &[DecodedValue::SignedInteger(*v)])?; }
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::SignedIntegerLE; ch.bit_count = 32; })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    for v in &v2 { w2.write_record(&cg2, &[DecodedValue::SignedInteger(*v)])?; }
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    let expected: Vec<i64> = v1.iter().chain(v2.iter()).copied().collect();
    assert_eq!(values.len(), expected.len());
    for (got, want) in values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::SignedInteger(g)) => assert_eq!(g, want),
            other => panic!("expected SignedInteger, got {:?}", other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_mixed_float_uint_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_mix1.mf4");
    let f2 = dir.join("mf4_merge_mix2.mf4");
    let out = dir.join("mf4_merge_mix_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    let t1 = w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::FloatLE; ch.bit_count = 64; ch.name = Some("Time".into()); })?;
    w1.add_channel(&cg1, Some(&t1), |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.bit_count = 32; ch.name = Some("Value".into()); })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    for i in 0..3u64 {
        w1.write_record(&cg1, &[DecodedValue::Float(i as f64 * 0.1), DecodedValue::UnsignedInteger(i)])?;
    }
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    let t2 = w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::FloatLE; ch.bit_count = 64; ch.name = Some("Time".into()); })?;
    w2.add_channel(&cg2, Some(&t2), |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.bit_count = 32; ch.name = Some("Value".into()); })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    for i in 3..6u64 {
        w2.write_record(&cg2, &[DecodedValue::Float(i as f64 * 0.1), DecodedValue::UnsignedInteger(i)])?;
    }
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1, "matching mixed groups must concatenate");
    let channels = groups[0].channels();
    let times = channels[0].values()?;
    let vals = channels[1].values()?;
    assert_eq!(times.len(), 6);
    assert_eq!(vals.len(), 6);
    for i in 0..6u64 {
        match &times[i as usize] {
            Some(DecodedValue::Float(g)) => assert_eq!(g.to_bits(), (i as f64 * 0.1).to_bits()),
            other => panic!("expected Float at {}, got {:?}", i, other),
        }
        match &vals[i as usize] {
            Some(DecodedValue::UnsignedInteger(g)) => assert_eq!(*g, i),
            other => panic!("expected UnsignedInteger at {}, got {:?}", i, other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_fixed_bytearray_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_ba1.mf4");
    let f2 = dir.join("mf4_merge_ba2.mf4");
    let out = dir.join("mf4_merge_ba_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let p1: Vec<[u8; 8]> = vec![[1, 2, 3, 4, 5, 6, 7, 8], [9, 10, 11, 12, 13, 14, 15, 16]];
    let p2: Vec<[u8; 8]> = vec![[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11]];

    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 64;
        ch.channel_type = 0;
    })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    for p in &p1 { w1.write_record(&cg1, &[DecodedValue::ByteArray(p.to_vec())])?; }
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 64;
        ch.channel_type = 0;
    })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    for p in &p2 { w2.write_record(&cg2, &[DecodedValue::ByteArray(p.to_vec())])?; }
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1, "matching ByteArray groups must concatenate");
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    let expected: Vec<&[u8; 8]> = p1.iter().chain(p2.iter()).collect();
    assert_eq!(values.len(), expected.len());
    for (got, want) in values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b.as_slice(), &want[..]),
            other => panic!("expected ByteArray, got {:?}", other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_different_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_a.mf4");
    let f2 = dir.join("mf4_merge_b.mf4");
    let out = dir.join("mf4_merge_out_diff.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    // file 1 with channel A
    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.name = Some("A".into()); })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    w1.write_record(&cg1, &[DecodedValue::UnsignedInteger(1)])?;
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    // file 2 with channel B
    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.name = Some("B".into()); })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    w2.write_record(&cg2, &[DecodedValue::UnsignedInteger(2)])?;
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 2);

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

fn write_vlsd_file<P: AsRef<std::path::Path>>(
    path: P,
    data_type: DataType,
    name: &str,
    payloads: &[Vec<u8>],
) -> Result<(), MdfError> {
    let path = path.as_ref().to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let dt = data_type.clone();
    let nm = name.to_string();
    w.add_channel(&cg, None, |ch| {
        ch.data_type = dt;
        ch.channel_type = 1;
        ch.data = 1;
        ch.bit_offset = 0;
        ch.byte_offset = 0;
        ch.bit_count = 64;
        ch.name = Some(nm);
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    for p in payloads {
        let value = match data_type {
            DataType::StringUtf8 | DataType::StringLatin1
            | DataType::StringUtf16LE | DataType::StringUtf16BE => {
                DecodedValue::String(String::from_utf8(p.clone()).expect("utf8 test payload"))
            }
            _ => DecodedValue::ByteArray(p.clone()),
        };
        w.write_record(&cg, &[value])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()
}

#[test]
fn merge_vlsd_string_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_vs1.mf4");
    let f2 = dir.join("mf4_merge_vs2.mf4");
    let out = dir.join("mf4_merge_vs_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let s1: Vec<&str> = vec!["alpha", "bravo"];
    let s2: Vec<&str> = vec!["charlie", "delta", "echo"];
    let p1: Vec<Vec<u8>> = s1.iter().map(|s| s.as_bytes().to_vec()).collect();
    let p2: Vec<Vec<u8>> = s2.iter().map(|s| s.as_bytes().to_vec()).collect();

    write_vlsd_file(&f1, DataType::StringUtf8, "Msg", &p1)?;
    write_vlsd_file(&f2, DataType::StringUtf8, "Msg", &p2)?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1, "matching VLSD string groups must concatenate");
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    let expected: Vec<&str> = s1.iter().chain(s2.iter()).copied().collect();
    assert_eq!(values.len(), expected.len());
    for (got, want) in values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::String(s)) => assert_eq!(s, want),
            other => panic!("expected String, got {:?}", other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_vlsd_bytearray_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_vb1.mf4");
    let f2 = dir.join("mf4_merge_vb2.mf4");
    let out = dir.join("mf4_merge_vb_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    let p1: Vec<Vec<u8>> = vec![vec![1, 2], vec![3, 4, 5, 6]];
    let p2: Vec<Vec<u8>> = vec![vec![], vec![7], vec![8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]];

    write_vlsd_file(&f1, DataType::ByteArray, "Frame", &p1)?;
    write_vlsd_file(&f2, DataType::ByteArray, "Frame", &p2)?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1, "matching VLSD bytearray groups must concatenate");
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    let expected: Vec<&Vec<u8>> = p1.iter().chain(p2.iter()).collect();
    assert_eq!(values.len(), expected.len());
    for (got, want) in values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b, *want),
            other => panic!("expected ByteArray, got {:?}", other),
        }
    }

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}
