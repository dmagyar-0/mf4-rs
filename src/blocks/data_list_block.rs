use crate::blocks::common::BlockHeader;
use crate::blocks::common::BlockParse;
use crate::error::MdfError;

/// DLBLOCK: Data List Block (ordered list of data blocks for signal/reduction)
pub struct DataListBlock {
    pub header: BlockHeader,
    pub next: u64,         // link to next DLBLOCK
    pub data_links: Vec<u64>,  // list of offsets to DT/RD/DV/RV/SDBLOCKs
}

impl BlockParse<'_> for DataListBlock {
    const ID: &'static str = "##DL";
    fn from_bytes(bytes: &[u8]) -> Result<Self, MdfError> {

        let header = Self::parse_header(bytes)?;
        
        let expected_bytes = 24 + (header.links_nr as usize * 8);
        if bytes.len() < expected_bytes {
            return Err(MdfError::TooShortBuffer {
                actual: bytes.len(),
                expected: expected_bytes,
                file: file!(), line: line!(),
            });
        }
        // Parse links: first is 'next', then data links
        let mut off = 24;
        let next = u64::from_le_bytes(bytes[off..off+8].try_into().unwrap());
        off += 8;

        // Remaining links all point to data blocks
        let link_count = header.links_nr as usize;
        let mut data_links = Vec::with_capacity(link_count - 1);
        for _ in 1..link_count {
            let l = u64::from_le_bytes(bytes[off..off+8].try_into().unwrap());
            data_links.push(l);
            off += 8;
        }

        Ok(DataListBlock { header, next, data_links })
    }
}
