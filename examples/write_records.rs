use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    let path = "write_records_example.mf4";

    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Value".into());
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    let records: Vec<Vec<DecodedValue>> = (0u64..5)
        .map(|i| vec![DecodedValue::UnsignedInteger(i)])
        .collect();
    let slices: Vec<&[DecodedValue]> = records.iter().map(|r| r.as_slice()).collect();
    writer.write_records(&cg_id, slices)?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    let mdf = MDF::from_file(path)?;
    let values = mdf.channel_groups()[0].channels()[0].values()?;
    println!("{} records written", values.len());
    Ok(())
}
