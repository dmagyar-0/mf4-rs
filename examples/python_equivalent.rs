use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;

#[path = "python_equivalent/const_sigs.rs"]
mod const_sigs;
use const_sigs::{SIG_LIST, RAW_MSG};

use std::env;
use std::time::Instant;

const NUM_SAMPLES: usize = 10_000_000;

fn dump_sig_list(fname: &str) -> Result<(), MdfError> {
    let mdf = MDF::from_file(fname)?;
    for group in mdf.channel_groups() {
        for ch in group.channels() {
            if let Some(name) = ch.name()? {
                if name != "t" {
                    println!("{}", name);
                }
            }
        }
    }
    Ok(())
}

fn read_test_signal(fname: &str, signame: &str) -> Result<(), MdfError> {
    let mdf = MDF::from_file(fname)?;
    for group in mdf.channel_groups() {
        for ch in group.channels() {
            if let Some(name) = ch.name()? {
                if name == signame {
                    let values = ch.values()?;
                    println!("{} values", values.len());
                }
            }
        }
    }
    println!("Done!");
    Ok(())
}

fn write_test() -> Result<(), MdfError> {
    println!("Writing test mdf4...");
    let mut writer = MdfWriter::new_with_capacity("asammdf_test.mf4", 4 * 1024 * 1024)?;
    writer.init_mdf_file()?;
    let cg = writer.add_channel_group(None, |_| {})?;
    let t_id = writer.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("t".into());
    })?;
    writer.set_time_channel(&t_id)?;
    writer.add_channel(&cg, Some(&t_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 32;
        ch.name = Some("FloatLE".into());
    })?;

    writer.start_data_block_for_cg(&cg, 0)?;
    for i in 0..NUM_SAMPLES {
        let ts = 100_000_000.0 + i as f64 * 1000.0;
        writer.write_record(
            &cg,
            &[DecodedValue::Float(ts), DecodedValue::Float(i as f64)],
        )?;
    }
    writer.finish_data_block(&cg)?;
    writer.finalize()?;
    println!("Done!");
    Ok(())
}



fn write_test_signals() -> Result<(), MdfError> {
    let mut writer =
        MdfWriter::new_with_capacity("asammdf_write_test_signals.tmp.mf4", 4 * 1024 * 1024)?;
    writer.init_mdf_file()?;
    let cg = writer.add_channel_group(None, |_| {})?;
    let t_id = writer.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("t".into());
    })?;
    writer.set_time_channel(&t_id)?;

    let mut prev_cn = t_id.clone();
    for sig in SIG_LIST {
        let cn_id = writer.add_channel(&cg, Some(&prev_cn), |ch| {
            ch.data_type = sig.data_type.clone();
            ch.bit_count = sig.bit_count;
            ch.name = Some(sig.name.into());
        })?;
        prev_cn = cn_id;
    }

    writer.start_data_block_for_cg(&cg, 0)?;
    let mut rec = Vec::with_capacity(SIG_LIST.len() + 1);
    for i in 0..NUM_SAMPLES {
        let ts = 100_000_000.0 + i as f64 * 1000.0;
        rec.clear();
        rec.push(DecodedValue::Float(ts));
        for s in SIG_LIST {
            match s.data_type {
                DataType::FloatLE | DataType::FloatBE => {
                    rec.push(DecodedValue::Float(s.float_val as f64));
                }
                _ => {
                    rec.push(DecodedValue::UnsignedInteger(s.int_val as u64));
                }
            }
        }
        writer.write_record(&cg, &rec)?;
    }
    writer.finish_data_block(&cg)?;
    writer.finalize()?;
    println!("Done!");
    Ok(())
}

fn write_test_bytes() -> Result<(), MdfError> {
    let mut writer =
        MdfWriter::new_with_capacity("asammdf_write_test_frame.tmp.mf4", 4 * 1024 * 1024)?;
    writer.init_mdf_file()?;
    let cg = writer.add_channel_group(None, |_| {})?;
    let t_id = writer.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("t".into());
    })?;
    writer.set_time_channel(&t_id)?;
    writer.add_channel(&cg, Some(&t_id), |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 512;
        ch.name = Some("CAN_DataBytes".into());
    })?;

    writer.start_data_block_for_cg(&cg, 0)?;
    for i in 0..NUM_SAMPLES {
        let ts = 100_000_000.0 + i as f64 * 1000.0;
        writer.write_record(
            &cg,
            &[DecodedValue::Float(ts), DecodedValue::ByteArray(RAW_MSG.to_vec())],
        )?;
    }
    writer.finish_data_block(&cg)?;
    writer.finalize()?;
    println!("Done!");
    Ok(())
}

fn main() -> Result<(), MdfError> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Please supply the required arguments!");
        return Ok(());
    }

    match args[1].as_str() {
        "read" => {
            if args.len() < 4 {
                eprintln!("Please supply a filename and signal name!");
            } else {
                let start = Instant::now();
                read_test_signal(&args[2], &args[3])?;
                println!("Completed in {:?}", start.elapsed());
            }
        }
        "write" => {
            let start = Instant::now();
            write_test()?;
            println!("Completed in {:?}", start.elapsed());
        }
        "write_signals" => {
            let start = Instant::now();
            write_test_signals()?;
            println!("Completed in {:?}", start.elapsed());
        }
        "write_frame" => {
            let start = Instant::now();
            write_test_bytes()?;
            println!("Completed in {:?}", start.elapsed());
        }
        "dump_signals" => {
            let start = Instant::now();
            dump_sig_list(&args[2])?;
            println!("Completed in {:?}", start.elapsed());
        }
        _ => {
            eprintln!("Unknown command");
        }
    }
    Ok(())
}
