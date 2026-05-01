// Variable-length signal data (##SD / ##DL) writing for MdfWriter
//
// VLSD payloads are buffered in memory while the parent DT block is being
// written, and only emitted to disk at `finish_signal_data_block` time. This
// keeps the parent DT block contiguous on disk: ##SD blocks live AFTER the
// DT block, not inside it.
use super::*;
use crate::blocks::common::BlockHeader;
use crate::blocks::data_list_block::DataListBlock;

/// Maximum payload size of a single ##SD fragment before we split into a new
/// fragment and chain via ##DL. Mirrors the DT splitting threshold used in
/// `data.rs`.
const MAX_SD_BLOCK_SIZE: u64 = 4 * 1024 * 1024;

impl MdfWriter {
    /// Begin recording a ##SD chain for the given VLSD channel.
    ///
    /// VLSD entries appended via [`write_signal_data`] are buffered in memory
    /// until [`finish_signal_data_block`] is called; the actual ##SD blocks
    /// are emitted to disk only on finish, after the parent DT block has
    /// been closed. This keeps the on-disk DT block contiguous.
    pub fn start_signal_data_block(&mut self, cn_id: &str) -> Result<(), MdfError> {
        if self.sd_buffers.contains_key(cn_id) {
            return Err(MdfError::BlockSerializationError(
                "signal data block already open for this channel".into(),
            ));
        }
        self.sd_buffers.insert(cn_id.to_string(), Vec::new());
        Ok(())
    }

    /// Buffer one VLSD entry for the channel.
    pub fn write_signal_data(&mut self, cn_id: &str, payload: &[u8]) -> Result<(), MdfError> {
        let buf = self.sd_buffers.get_mut(cn_id).ok_or_else(|| {
            MdfError::BlockSerializationError(
                "no open signal data block for this channel".into(),
            )
        })?;
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        Ok(())
    }

    /// Emit the buffered VLSD entries as one or more ##SD blocks (chained via
    /// ##DL when the payload exceeds [`MAX_SD_BLOCK_SIZE`]) and patch the
    /// channel's `data` link to point at the resulting block.
    ///
    /// Must be called after the parent DT block has been finalized so the
    /// emitted ##SD blocks land cleanly after the DT block in the file.
    pub fn finish_signal_data_block(&mut self, cn_id: &str) -> Result<(), MdfError> {
        let buffer = self.sd_buffers.remove(cn_id).ok_or_else(|| {
            MdfError::BlockSerializationError(
                "no open signal data block for this channel".into(),
            )
        })?;
        let cn_pos = self.get_block_position(cn_id).ok_or_else(|| {
            MdfError::BlockLinkError(format!("Channel block '{}' not found", cn_id))
        })?;
        let cn_data_link_offset = 64u64; // ChannelBlock.data field

        // Split the buffered VLSD stream into ##SD fragments at u32-length
        // entry boundaries so no entry straddles a fragment.
        let max_payload = (MAX_SD_BLOCK_SIZE - 24) as usize;
        let mut fragments: Vec<&[u8]> = Vec::new();
        let mut start = 0usize;
        let mut cursor = 0usize;
        while cursor < buffer.len() {
            // Read entry length prefix.
            if cursor + 4 > buffer.len() {
                return Err(MdfError::BlockSerializationError(
                    "VLSD buffer truncated mid-length-prefix".into(),
                ));
            }
            let len =
                u32::from_le_bytes(buffer[cursor..cursor + 4].try_into().unwrap()) as usize;
            let entry_end = cursor + 4 + len;
            if entry_end > buffer.len() {
                return Err(MdfError::BlockSerializationError(
                    "VLSD buffer truncated mid-payload".into(),
                ));
            }
            // If adding this entry would exceed the fragment cap, flush the
            // current fragment first (without the new entry).
            if entry_end - start > max_payload && cursor > start {
                fragments.push(&buffer[start..cursor]);
                start = cursor;
            }
            cursor = entry_end;
        }
        if start < buffer.len() {
            fragments.push(&buffer[start..]);
        }
        if fragments.is_empty() {
            // Even an empty VLSD chain still needs an ##SD block so the
            // channel.data link is valid. Emit a single zero-length ##SD.
            fragments.push(&[]);
        }

        // Emit the ##SD fragments in order.
        let mut sd_positions: Vec<u64> = Vec::with_capacity(fragments.len());
        let mut sd_sizes: Vec<u64> = Vec::with_capacity(fragments.len());
        for fragment in &fragments {
            let block_len = 24u64 + fragment.len() as u64;
            let sd_id = self.next_sd_id();
            let header = BlockHeader {
                id: "##SD".to_string(),
                reserved0: 0,
                block_len,
                links_nr: 0,
            };
            let mut bytes = header.to_bytes()?;
            bytes.extend_from_slice(fragment);
            let pos = self.write_block_with_id(&bytes, &sd_id)?;
            sd_positions.push(pos);
            sd_sizes.push(block_len);
        }

        if sd_positions.len() == 1 {
            self.update_link(cn_pos + cn_data_link_offset, sd_positions[0])?;
        } else {
            // SD fragments have variable sizes (split at entry boundaries to
            // keep entries from straddling fragments), so we cannot use the
            // equal-length form. Emit `flags = 0` with a per-fragment virtual
            // offset list — this is what spec-conformant random-access readers
            // (e.g. asammdf, Vector) use to resolve the inline VLSD offsets in
            // parent records back to a fragment + intra-fragment position.
            let mut virtual_offsets: Vec<u64> = Vec::with_capacity(sd_sizes.len());
            let mut acc: u64 = 0;
            for &block_len in &sd_sizes {
                virtual_offsets.push(acc);
                // Each fragment contributes (block_len - 24) bytes of data
                // section to the concatenated stream.
                acc = acc.saturating_add(block_len.saturating_sub(24));
            }
            let dl_id = self.next_dl_id();
            let dl_block = DataListBlock::new_variable(sd_positions, virtual_offsets);
            let dl_bytes = dl_block.to_bytes()?;
            let _ = self.write_block_with_id(&dl_bytes, &dl_id)?;
            let dl_pos = self.get_block_position(&dl_id).unwrap();
            self.update_link(cn_pos + cn_data_link_offset, dl_pos)?;
        }
        Ok(())
    }

    fn next_sd_id(&self) -> String {
        let n = self
            .block_positions
            .keys()
            .filter(|k| k.starts_with("sd_"))
            .count();
        format!("sd_{}", n)
    }

    fn next_dl_id(&self) -> String {
        let n = self
            .block_positions
            .keys()
            .filter(|k| k.starts_with("dl_"))
            .count();
        format!("dl_{}", n)
    }
}
