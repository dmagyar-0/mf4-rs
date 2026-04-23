use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    // Assumes `write_file` has been run to create the file.
    let path = "example.mf4";

    // Option 1: build the layout directly from a path.
    let layout = FileLayout::from_file(path)?;

    // Option 2: build it through an MDF instance you already hold.
    let mdf = MDF::from_file(path)?;
    let _layout_via_mdf = mdf.file_layout()?;

    println!("===== Flat listing =====");
    println!("{}", layout.to_text());

    println!("===== Link tree =====");
    println!("{}", layout.to_tree());

    // Persist representations to disk.
    layout.write_text_to_file("example.layout.txt")?;
    layout.write_tree_to_file("example.layout.tree.txt")?;
    layout.write_json_to_file("example.layout.json")?;

    println!(
        "Wrote example.layout.txt, example.layout.tree.txt, example.layout.json ({} blocks, {} gaps)",
        layout.blocks.len(),
        layout.gaps.len()
    );
    Ok(())
}
