use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::error::MdfError;
use mf4_rs::cut::cut_mdf_by_time;

fn main() -> Result<(), MdfError> {
    let input = "cut_example_input.mf4";
    let output = "cut_example_output.mf4";

    // create a simple MF4 file with a time channel and a value channel
    let mut writer = MdfWriter::new(input)?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Val".into());
        ch.bit_count = 32;
    })?;
    writer.start_data_block_for_cg(&cg_id, 0)?;
    for i in 0u64..10 {
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

    // cut between 0.3 and 0.6 seconds
    cut_mdf_by_time(input, output, 0.3, 0.6)?;

    println!("Created {} and {}", input, output);
    Ok(())
}
