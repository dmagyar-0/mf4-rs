use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::data_list_block::DataListBlock;
use crate::blocks::signal_data_block::SignalDataBlock;
use crate::blocks::common::BlockParse;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::error::MdfError;

/// A channel with lazy access to its raw record bytes (fixed-length or VLSD).
#[derive(Debug)]
pub struct RawChannel {
    pub block:  ChannelBlock,
}

impl<'a> RawChannel {

    pub fn records(
        &self,
        data_group: &'a RawDataGroup,
        channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Result<Box<dyn Iterator<Item = Result<&'a [u8], MdfError>> + 'a>, MdfError> {
        // 1) VLSD path: channel has its own data pointer => SD/DL chain
        if self.block.channel_type == 1 {
            // Capture the file bytes and channel pointer
            let bytes = mmap;
            let mut next_addr = self.block.data;
            let mut data_links = Vec::new();
            let mut link_idx = 0;
            let mut current_sdb: Option<SignalDataBlock> = None;
            let mut sdb_pos = 0;

            // Build a from_fn iterator carrying that mutable state
            let vlsd_iter = std::iter::from_fn(move || -> Option<Result<&'a [u8], MdfError>> {
                loop {
                    // 1) Yield from an open SDBLOCK if any
                    if let Some(sdb) = &current_sdb {
                        let buf = sdb.data;
                        if sdb_pos + 4 <= buf.len() {
                            let len = u32::from_le_bytes(
                                buf[sdb_pos..sdb_pos+4].try_into().unwrap()
                            ) as usize;
                            let start = sdb_pos + 4;
                            let end = start + len;
                            if end > buf.len() {
                                return Some(Err(MdfError::TooShortBuffer {
                                    actual:   buf.len(),
                                    expected: end,
                                    file:     file!(),
                                    line:     line!(),
                                }));
                            }
                            let slice = &buf[start..end];
                            sdb_pos = end;
                            return Some(Ok(slice));
                        }
                        // exhausted
                        current_sdb = None;
                    }

                    // 2) Next link in current DL batch?
                    if link_idx < data_links.len() {
                        let frag_addr = data_links[link_idx];
                        link_idx += 1;
                        let off = frag_addr as usize;
                        // SDBLOCK
                        return Some(
                            SignalDataBlock::from_bytes(&bytes[off..])
                                .map(|sdb| {
                                    // prepare to yield from it next iteration
                                    current_sdb = Some(sdb);
                                    sdb_pos = 0;
                                    // this iteration produces *no* record,
                                    // but we want to loop again immediately:
                                    // so we signal "no yield" with None,
                                    // but map that None → loop continue
                                    // (we'll handle it below)
                                    None
                                })
                                .map_err(Into::into)
                                .and_then(|opt| opt.ok_or_else(|| unreachable!()))
                        );
                    }

                    // 3) If we have a next_addr, peek its ID to decide what it is
                    if next_addr != 0 {
                        let off = next_addr as usize;
                        // read the 4-byte ID
                        let id = &bytes[off..off+4];
                        match id {
                            b"##DL" => {
                                // Data List Block
                                match DataListBlock::from_bytes(&bytes[off..]) {
                                    Ok(dl) => {
                                        data_links = dl.data_links.clone();
                                        link_idx = 0;
                                        next_addr = dl.next;
                                        continue;  // back to loop start
                                    }
                                    Err(e) => return Some(Err(e)),
                                }
                            }
                            b"##SD" => {
                                // Direct Signal Data Block
                                match SignalDataBlock::from_bytes(&bytes[off..]) {
                                    Ok(sdb) => {
                                        current_sdb = Some(sdb);
                                        sdb_pos = 0;
                                        next_addr = 0; // no list chain
                                        continue;
                                    }
                                    Err(e) => return Some(Err(e)),
                                }
                            }
                            other => {
                                // unexpected block type
                                return Some(Err(MdfError::BlockIDError {
                                    actual:   String::from_utf8_lossy(other).into(),
                                    expected: "##DL or ##SD".to_string(),
                                }));
                            }
                        }
                    }

                    // 4) Done
                    return None;
                }
            });

            return Ok(Box::new(vlsd_iter));
        }

        // Compute the size of each record:
        let record_id_len    = data_group.block.record_id_len as usize;
        let sample_byte_len  = channel_group.block.samples_byte_nr as usize;
        let record_size      = record_id_len + sample_byte_len;

        // Gather all DataBlock fragments (DT, DV or DZ):
        let blocks = data_group.data_blocks(mmap)?;

        // Build a single iterator that:
        //  - goes block by block
        //  - trims any partial record at the end of each block
        //  - yields & [u8] of length `record_size`
        let iter = blocks.into_iter().flat_map(move |data_block| {
            // For DZBLOCK you already unzipped into DataBlock, so here data_block.data
            let raw = data_block.data;
            let valid_len = (raw.len() / record_size) * record_size;
            // `chunks_exact` returns an iterator of &[u8] each exactly record_size
            raw[..valid_len].chunks_exact(record_size)
                // wrap each slice in Ok(...) so the overall Iterator<Item=Result<_,_>>
                .map(Ok)
                // If you wanted to handle an unexpected remainder, you could check raw.len() % record_size != 0 here.
        });

        Ok(Box::new(iter))
    }
}

