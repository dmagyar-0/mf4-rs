//! Minimal utilities for reading and writing ASAM MDF 4 files.
//!
//! The crate exposes a high level API under [`api`] to inspect existing
//! recordings as well as a [`writer::MdfWriter`] to generate new files.  Only a
//! fraction of the MDF 4 specification is implemented.

pub mod blocks;
pub mod error;
pub mod writer;
pub mod cut;

pub mod merge;

pub mod parsing {
    pub mod decoder;
    pub mod mdf_file;
    pub mod raw_channel_group;
    pub mod raw_data_group;
    pub mod raw_channel;
    pub mod source_info;
}

pub mod api {
    pub mod mdf;
    pub mod channel_group;
    pub mod channel;
}
