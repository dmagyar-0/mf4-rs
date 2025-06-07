use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    // Write phase
    let mut writer = MdfWriter::new("write_read_example.mf4")?;
    let (_id, _hd) = writer.init_mdf_file()?;
    let dg_id = writer.add_data_group(None)?;
    let cg_id = writer.add_channel_group(&dg_id, None)?;

    let mut ch = ChannelBlock::default();
    ch.byte_offset = 0;
    ch.bit_count = 32;
    ch.data_type = DataType::UnsignedIntegerLE;
    writer.add_channel(&cg_id, None, Some("Signal"), 0, 32)?;

    writer.start_data_block(&dg_id, &cg_id, 0, &[ch.clone()])?;
    for i in 0u32..1_000 {
        writer.write_record(&cg_id, &[DecodedValue::UnsignedInteger(i.into())])?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Read phase using the high level API
    let mdf = MDF::from_file("write_read_example.mf4")?;
    for group in mdf.channel_groups() {
        for channel in group.channels() {
            if let Some(name) = channel.name()? { println!("Channel: {}", name); }
            let values = channel.values()?;
            println!("Total records: {}", values.len());
            println!("First value: {:?}", values.first());
            println!("Last value: {:?}", values.last());
        }
    }
    Ok(())
}
