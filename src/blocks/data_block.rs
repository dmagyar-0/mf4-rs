use crate::blocks::common::BlockHeader;
use crate::blocks::common::BlockParse;
use crate::error::MdfError;

#[derive(Debug)]
pub struct DataBlock<'a> {
    pub header: BlockHeader,
    pub data: &'a [u8],
}

impl<'a> BlockParse<'a> for DataBlock<'a> {
    const ID: &'static str = "##DT";
    fn from_bytes(bytes: &'a[u8]) -> Result<Self, MdfError> {

        let header = Self::parse_header(bytes)?;

        let data_len = (header.block_len as usize).saturating_sub(24);
        let expected_bytes = 24 + data_len;
        if bytes.len() < expected_bytes {
            return Err(MdfError::TooShortBuffer {
                actual:   bytes.len(),
                expected: expected_bytes,
                file:     file!(),
                line:     line!(),
            });
        }
        let data = &bytes[24..24 + data_len];
        Ok(Self { header, data })
    }
}
impl<'a> DataBlock<'a> {
    /// Iterate over raw records of fixed size.
    /// If the data block contains padding at the end, it’s your caller’s responsibility to trim that.
    pub fn records(&self, record_size: usize) -> impl Iterator<Item = &'a [u8]> {
        self.data.chunks_exact(record_size)
    }
}
