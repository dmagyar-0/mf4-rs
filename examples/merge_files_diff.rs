use mf4_rs::writer::MdfWriter;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::blocks::common::DataType;
use mf4_rs::merge::merge_files;
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    let input1 = "merge_diff1.mf4";
    let input2 = "merge_diff2.mf4";
    let output = "merge_diff_result.mf4";

    for path in [input1, input2, output] {
        let _ = std::fs::remove_file(path);
    }

    // File 1 with channel "A"
    let mut w1 = MdfWriter::new(input1)?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("A".into());
    })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    w1.write_record(&cg1, &[DecodedValue::UnsignedInteger(1)])?;
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    // File 2 with channel "B" (different name)
    let mut w2 = MdfWriter::new(input2)?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("B".into());
    })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    w2.write_record(&cg2, &[DecodedValue::UnsignedInteger(2)])?;
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    // Merge - groups differ so expect two groups in output
    merge_files(output, input1, input2)?;

    // Inspect using parser API
    let mdf = MDF::from_file(output)?;
    println!("Merged file has {} channel group(s)", mdf.channel_groups().len());
    for (i, group) in mdf.channel_groups().iter().enumerate() {
        println!(" Group {}: {} channel(s)", i + 1, group.channels().len());
        for ch in group.channels() {
            println!("  {:?} -> {:?}", ch.name()?, ch.values()?);
        }
    }

    Ok(())
}
