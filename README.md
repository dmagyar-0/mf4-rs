# mf4-rs

This crate provides basic read and write support for the ASAM MDF 4 format.

## Examples

Run `cargo run --example cut_file` to create a small MF4 file and cut it
between two timestamps. The resulting file can be verified with tools such as
`asammdf`.

Run `cargo run --example write_records` to generate a file using
`MdfWriter::write_records` for appending multiple records at once.

## API Highlights

- `MdfWriter::write_record` – append a single record to a data block.
- `MdfWriter::write_records` – append a series of records in one call.

