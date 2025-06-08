//! Demonstrates writing data using multiple DT blocks per channel group.
//! Data blocks automatically roll over after 4 MiB and a data list block
//! records all DT fragments for the group.

use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    // Create writer and base file structure
    let mut writer = MdfWriter::new("multi_dt.mf4")?;
    let (_id, _hd) = writer.init_mdf_file()?;

    // Single data group with two channel groups
    let dg_id = writer.add_data_group(None)?;
    let cg_block = ChannelGroupBlock::default();
    let cg1_id = writer.add_channel_group(&dg_id, None, &cg_block)?;
    let cg2_id = writer.add_channel_group(&dg_id, Some(&cg1_id), &cg_block)?;

    // Define one channel in each channel group
    let mut ch1 = ChannelBlock::default();
    ch1.byte_offset = 0;
    ch1.bit_count = 32;
    ch1.data_type = DataType::UnsignedIntegerLE;
    ch1.name = Some("Group1_Signal".to_string());
    writer.add_channel(&cg1_id, None, &ch1)?;

    let mut ch2 = ChannelBlock::default();
    ch2.byte_offset = 0;
    ch2.bit_count = 32;
    ch2.data_type = DataType::UnsignedIntegerLE;
    ch2.name = Some("Group2_Signal".to_string());
    writer.add_channel(&cg2_id, None, &ch2)?;

    // Start DT blocks for each channel group
    writer.start_data_block(&dg_id, &cg1_id, 0, &[ch1.clone()])?;
    writer.start_data_block(&dg_id, &cg2_id, 0, &[ch2.clone()])?;

    // Append many records to trigger rollover into additional DT blocks
    for i in 0u32..1_100_000 {
        writer.write_record(&cg1_id, &[DecodedValue::UnsignedInteger(i.into())])?;
        writer.write_record(&cg2_id, &[DecodedValue::UnsignedInteger((i * 2).into())])?;
    }

    // Finalize each data block (creates a DL block when multiple DTs were written)
    writer.finish_data_block(&cg1_id)?;
    writer.finish_data_block(&cg2_id)?;

    writer.finalize()
}
