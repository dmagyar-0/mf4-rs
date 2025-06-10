//! Implementation of the MdfWriter struct split across several submodules

use std::fs::File;
use std::collections::HashMap;

use crate::blocks::channel_block::ChannelBlock;
use crate::error::MdfError;

mod io;
mod init;
mod data;

/// Helper structure tracking an open DTBLOCK during writing
struct OpenDataBlock {
    dg_id: String,
    dt_id: String,
    start_pos: u64,
    record_size: usize,
    record_count: u64,
    record_id_len: usize,
    channels: Vec<ChannelBlock>,
    dt_ids: Vec<String>,
    dt_positions: Vec<u64>,
    dt_sizes: Vec<u64>,
}

/// Writer for MDF blocks, ensuring 8-byte alignment and zero padding.
/// Tracks block positions and supports updating links at a later stage.
pub struct MdfWriter {
    file: File,
    offset: u64,
    block_positions: HashMap<String, u64>,
    open_dts: HashMap<String, OpenDataBlock>,
    dt_counter: usize,
    last_dg: Option<String>,
    cg_to_dg: HashMap<String, String>,
    cg_offsets: HashMap<String, usize>,
    cg_channels: HashMap<String, Vec<ChannelBlock>>,
    channel_map: HashMap<String, (String, usize)>,
}
