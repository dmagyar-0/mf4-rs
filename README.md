# mf4-rs

`mf4-rs` is a minimal library for working with ASAM MDF 4 measurement files.
It supports parsing existing files as well as writing new ones through a safe
API.  Only a subset of the standard is implemented but it is sufficient for
simple data logging and inspection tasks.

## Examples

Run `cargo run --example cut_file` to create a small MF4 file and cut it between
two timestamps. The resulting file can be verified with tools such as
`asammdf`.
When inspected with `asammdf` the trimmed output shows the expected
time values `0.3`, `0.4`, `0.5` and `0.6` alongside integer values
`3` to `6`.

Run `cargo run --example write_records` to generate a file using
`MdfWriter::write_records` for appending multiple records at once.

## Usage

Parsing a file is straightforward:

```rust
use mf4_rs::api::mdf::MDF;

let mdf = MDF::from_file("capture.mf4")?;
for group in mdf.channel_groups() {
    println!("channels: {}", group.channels().len());
}
```

Writing a file is done via `MdfWriter`:

```rust
use mf4_rs::writer::MdfWriter;

let mut writer = MdfWriter::new("out.mf4")?;
writer.init_mdf_file()?;
let cg = writer.add_channel_group(None, |_| {})?;
writer.add_channel(&cg, None, |_| {})?;
writer.start_data_block_for_cg(&cg, 0)?;
writer.finish_data_block(&cg)?;
writer.finalize()?;
```

## API Highlights

- `MdfWriter::write_record` – append a single record to a data block.
- `MdfWriter::write_records` – append a series of records in one call.

