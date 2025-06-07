use crate::blocks::common::BlockHeader;
use crate::blocks::common::BlockParse;
use crate::error::MdfError;

/// DLBLOCK: Data List Block (ordered list of data blocks for signal/reduction)
pub struct DataListBlock {
    pub header: BlockHeader,
    pub next: u64,             // link to next DLBLOCK
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

impl DataListBlock {
    /// Create a new DataListBlock referencing the provided data blocks.
    pub fn new(data_links: Vec<u64>) -> Self {
        let links_nr = data_links.len() as u64 + 1; // +1 for the 'next' link
        let block_len = 24 + links_nr * 8;
        let header = BlockHeader {
            id: "##DL".to_string(),
            reserved0: 0,
            block_len,
            links_nr,
        };
        Self { header, next: 0, data_links }
    }

    /// Serialize this DLBLOCK to bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MdfError> {
        if self.header.id != "##DL" {
            return Err(MdfError::BlockSerializationError(
                format!("DataListBlock must have ID '##DL', found '{}'", self.header.id)
            ));
        }

        let links_nr = self.data_links.len() as u64 + 1;
        let block_len = 24 + links_nr * 8;

        if self.header.links_nr != links_nr {
            return Err(MdfError::BlockSerializationError(
                format!("DataListBlock links_nr mismatch: header {} vs actual {}", self.header.links_nr, links_nr)
            ));
        }
        if self.header.block_len != block_len {
            return Err(MdfError::BlockSerializationError(
                format!("DataListBlock block_len mismatch: header {} vs actual {}", self.header.block_len, block_len)
            ));
        }

        let mut buf = Vec::with_capacity(block_len as usize);
        buf.extend_from_slice(&self.header.to_bytes()?);
        buf.extend_from_slice(&self.next.to_le_bytes());
        for link in &self.data_links {
            buf.extend_from_slice(&link.to_le_bytes());
        }
        Ok(buf)
    }
}
