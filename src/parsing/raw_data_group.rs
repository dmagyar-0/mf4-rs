use crate::error::MdfError;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::blocks::{
    data_block::DataBlock,
    data_group_block::DataGroupBlock,
    common::BlockHeader,
};
use crate::parsing::compression::{collect_fragments, coalesce, Fragment};
use std::sync::OnceLock;

#[derive(Debug)]
pub struct RawDataGroup {
    pub block: DataGroupBlock,
    pub channel_groups: Vec<RawChannelGroup>,
    /// Owned record bytes for groups whose data cannot be served zero-copy
    /// from the mmap (`##DZ`-compressed fragments, or `##DL` fragments that
    /// split records). Filled on first access and reused afterwards.
    pub(crate) coalesced: OnceLock<Vec<u8>>,
}

impl RawDataGroup {
    /// Create a data group wrapper (the decompression cache starts empty).
    pub fn new(block: DataGroupBlock, channel_groups: Vec<RawChannelGroup>) -> Self {
        Self { block, channel_groups, coalesced: OnceLock::new() }
    }

    /// Size in bytes of one record of this (sorted) data group, or 0 when
    /// it is not uniquely defined.
    pub fn record_size(&self) -> usize {
        match self.channel_groups.as_slice() {
            [cg] => self.block.record_id_len as usize
                + cg.block.samples_byte_nr as usize
                + cg.block.invalidation_bytes_nr as usize,
            _ => 0,
        }
    }

    /// Collect all data blocks referenced by this data group.
    ///
    /// Transparently follows `##DL` chains and `##HL` header lists and
    /// decompresses `##DZ` fragments. Plain, record-aligned data is returned
    /// zero-copy as slices of the mmap. Compressed data, or fragments that
    /// split a record across a boundary, are decoded once into an owned
    /// buffer cached on this group and returned as a single block.
    ///
    /// # Arguments
    /// * `mmap` - Memory mapped file containing the MDF data
    ///
    /// # Returns
    /// A vector of [`DataBlock`] objects or an [`MdfError`] if parsing fails.
    pub fn data_blocks<'a>(
        &'a self,
        mmap: &'a [u8],
    ) -> Result<Vec<DataBlock<'a>>, MdfError> {
        // Unsorted data groups interleave records of several channel groups
        // (each prefixed by a record id). Framing them as fixed-size records
        // of a single group silently mis-decodes the data, so refuse loudly.
        // Metadata access (names, channels) is unaffected — only record/data
        // access goes through here.
        if self.block.record_id_len > 0 && self.channel_groups.len() > 1 {
            return Err(MdfError::BlockSerializationError(
                "unsorted data groups (multiple channel groups per data group) are not supported"
                    .to_string(),
            ));
        }

        let synthetic = |data: &'a [u8]| DataBlock {
            header: BlockHeader {
                id: "##DT".to_string(),
                reserved0: 0,
                block_len: 24 + data.len() as u64,
                links_nr: 0,
            },
            data,
        };

        if let Some(buf) = self.coalesced.get() {
            return Ok(vec![synthetic(buf.as_slice())]);
        }

        let fragments = collect_fragments(mmap, self.block.data_block_addr, &["##DT", "##DV"])?;
        let record_size = self.record_size();
        let zero_copy = fragments.iter().all(|f| match f {
            Fragment::Raw(r) => record_size == 0 || r.len() % record_size == 0,
            Fragment::Dz(_) => false,
        });
        // A single trailing partial record in the last plain fragment is
        // tolerated (readers trim it), matching the previous behaviour.
        let only_tail_misaligned = record_size > 0
            && !fragments.is_empty()
            && fragments.iter().all(|f| matches!(f, Fragment::Raw(_)))
            && fragments[..fragments.len() - 1].iter().all(|f| match f {
                Fragment::Raw(r) => r.len() % record_size == 0,
                Fragment::Dz(_) => false,
            });

        if zero_copy || only_tail_misaligned {
            return Ok(fragments
                .into_iter()
                .map(|f| match f {
                    Fragment::Raw(r) => synthetic(r),
                    Fragment::Dz(_) => unreachable!("zero-copy path has no DZ"),
                })
                .collect());
        }

        let buf = coalesce(&fragments)?;
        let _ = self.coalesced.set(buf);
        Ok(vec![synthetic(self.coalesced.get().expect("just set").as_slice())])
    }
}
