use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    let mut writer = MdfWriter::new("large_data.mf4")?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;

    let mut prev: Option<String> = None;
    for idx in 0..4 {
        let id = writer.add_channel(&cg_id, prev.as_deref(), |ch| {
            ch.data_type = DataType::UnsignedIntegerLE;
            ch.name = Some(format!("Ch{}", idx + 1));
            ch.bit_count = 64;
        })?;
        prev = Some(id);
    }

    writer.start_data_block_for_cg(&cg_id, 0)?;
    for i in 0u64..150_000 {
        writer.write_record(
            &cg_id,
            &[
                DecodedValue::UnsignedInteger(i),
                DecodedValue::UnsignedInteger(i + 1),
                DecodedValue::UnsignedInteger(i + 2),
                DecodedValue::UnsignedInteger(i + 3),
            ],
        )?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;
    Ok(())
}
