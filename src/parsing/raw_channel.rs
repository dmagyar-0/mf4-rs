use crate::blocks::channel_block::ChannelBlock;
use crate::parsing::compression::{collect_fragments, coalesce, Fragment};
use std::sync::OnceLock;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::error::MdfError;

/// A channel with lazy access to its raw record bytes (fixed-length or VLSD).
#[derive(Debug)]
pub struct RawChannel {
    pub block:  ChannelBlock,
    /// Decompressed signal data for VLSD channels stored in `##DZ` blocks.
    sd_coalesced: OnceLock<Vec<u8>>,
}

impl RawChannel {
    pub fn new(block: ChannelBlock) -> Self {
        Self { block, sd_coalesced: OnceLock::new() }
    }

    /// Payload slices of this VLSD channel's signal data, one per fragment.
    /// Plain `##SD` fragments are borrowed from the mmap; if any fragment is
    /// compressed, the whole chain is inflated once and cached.
    pub fn signal_data_fragments<'a>(&'a self, mmap: &'a [u8]) -> Result<Vec<&'a [u8]>, MdfError> {
        if let Some(buf) = self.sd_coalesced.get() {
            return Ok(vec![buf.as_slice()]);
        }
        let fragments = collect_fragments(mmap, self.block.data, &["##SD"])?;
        if fragments.iter().all(|f| !f.is_compressed()) {
            return Ok(fragments
                .into_iter()
                .map(|f| match f {
                    Fragment::Raw(r) => r,
                    Fragment::Dz(_) => unreachable!(),
                })
                .collect());
        }
        let buf = coalesce(&fragments)?;
        let _ = self.sd_coalesced.set(buf);
        Ok(vec![self.sd_coalesced.get().expect("just set").as_slice()])
    }
}

impl<'a> RawChannel {

    /// Return an iterator over raw record bytes for this channel.
    ///
    /// The iterator yields a `Result` for each record and transparently handles
    /// both fixed-size and VLSD storage schemes.
    ///
    /// # Arguments
    /// * `data_group` - Parent data group owning the records
    /// * `channel_group` - Channel group this channel belongs to
    /// * `mmap` - Memory mapped MDF data
    ///
    /// # Returns
    /// An iterator over byte slices containing each raw record, or an
    /// [`MdfError`] if the underlying blocks could not be parsed.
    pub fn records(
        &'a self,
        data_group: &'a RawDataGroup,
        channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Result<Box<dyn Iterator<Item = Result<&'a [u8], MdfError>> + 'a>, MdfError> {
        // 1) VLSD path: channel has its own data pointer => SD chain
        //    (##SD, ##DL of ##SD, ##HL/##DZ-compressed variants).
        if self.block.channel_type == 1 && self.block.data != 0 {
            let fragments = self.signal_data_fragments(mmap)?;
            let mut frag_idx = 0usize;
            let mut pos = 0usize;
            let vlsd_iter = std::iter::from_fn(move || -> Option<Result<&'a [u8], MdfError>> {
                loop {
                    let buf: &'a [u8] = fragments.get(frag_idx)?;
                    if pos + 4 <= buf.len() {
                        let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
                        let start = pos + 4;
                        let end = match start.checked_add(len) {
                            Some(e) if e <= buf.len() => e,
                            _ => {
                                // Poison the iterator so a corrupt entry is
                                // reported exactly once.
                                frag_idx = usize::MAX;
                                return Some(Err(MdfError::TooShortBuffer {
                                    actual:   buf.len(),
                                    expected: start.saturating_add(len),
                                    file:     file!(),
                                    line:     line!(),
                                }));
                            }
                        };
                        pos = end;
                        return Some(Ok(&buf[start..end]));
                    }
                    frag_idx += 1;
                    pos = 0;
                }
            });
            return Ok(Box::new(vlsd_iter));
        }

        // Compute the size of each record:
        // Record structure: record_id + data_bytes + invalidation_bytes
        let record_id_len       = data_group.block.record_id_len as usize;
        let sample_byte_len     = channel_group.block.samples_byte_nr as usize;
        let invalidation_bytes  = channel_group.block.invalidation_bytes_nr as usize;
        let record_size         = record_id_len + sample_byte_len + invalidation_bytes;

        // A record size of zero (malformed channel group) would make the
        // chunking below divide by zero / panic — there are no records.
        if record_size == 0 {
            return Ok(Box::new(std::iter::empty()));
        }

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

