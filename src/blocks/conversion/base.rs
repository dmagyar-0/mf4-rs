use byteorder::{LittleEndian, ByteOrder};
use crate::blocks::common::{BlockHeader, BlockParse};
use crate::error::MdfError;
use super::types::ConversionType;

#[derive(Debug, Clone)]
pub struct ConversionBlock {
    pub header: BlockHeader,

    // Link section
    pub cc_tx_name: Option<u64>,
    pub cc_md_unit: Option<u64>,
    pub cc_md_comment: Option<u64>,
    pub cc_cc_inverse: Option<u64>,
    pub cc_ref: Vec<u64>,

    // Data
    pub cc_type: ConversionType,
    pub cc_precision: u8,
    pub cc_flags: u16,
    pub cc_ref_count: u16,
    pub cc_val_count: u16,
    pub cc_phy_range_min: Option<f64>,
    pub cc_phy_range_max: Option<f64>,
    pub cc_val: Vec<f64>,

    pub formula: Option<String>,
}

impl BlockParse<'_> for ConversionBlock {
    const ID: &'static str = "##CC";
    fn from_bytes(bytes: &[u8]) -> Result<Self, MdfError> {

        let header = Self::parse_header(bytes)?;

        let mut offset = 24;

        // Fixed links
        let cc_tx_name = read_link(bytes, &mut offset);
        let cc_md_unit = read_link(bytes, &mut offset);
        let cc_md_comment = read_link(bytes, &mut offset);
        let cc_cc_inverse = read_link(bytes, &mut offset);

        let fixed_links = 4;
        let additional_links = header.links_nr.saturating_sub(fixed_links);
        let mut cc_ref = Vec::with_capacity(additional_links as usize);
        for _ in 0..additional_links {
            cc_ref.push(read_u64(bytes, &mut offset)?);
        }

        // Basic fields
        let cc_type = ConversionType::from_u8(bytes[offset]);
        offset += 1;
        let cc_precision = bytes[offset];
        offset += 1;
        let cc_flags = LittleEndian::read_u16(&bytes[offset..offset + 2]);
        offset += 2;
        let cc_ref_count = LittleEndian::read_u16(&bytes[offset..offset + 2]);
        offset += 2;
        let cc_val_count = LittleEndian::read_u16(&bytes[offset..offset + 2]);
        offset += 2;

        let cc_phy_range_min = if cc_flags & 0b10 != 0 {
            let val = f64::from_bits(read_u64(bytes, &mut offset)?);
            Some(val)
        } else {
            None
        };

        let cc_phy_range_max = if cc_flags & 0b10 != 0 {
            let val = f64::from_bits(read_u64(bytes, &mut offset)?);
            Some(val)
        } else {
            None
        };

        let mut cc_val = Vec::with_capacity(cc_val_count as usize);
        for _ in 0..cc_val_count {
            let val = f64::from_bits(read_u64(bytes, &mut offset)?);
            cc_val.push(val);
        }

        Ok(Self {
            header,
            cc_tx_name,
            cc_md_unit,
            cc_md_comment,
            cc_cc_inverse,
            cc_ref,
            cc_type,
            cc_precision,
            cc_flags,
            cc_ref_count,
            cc_val_count,
            cc_phy_range_min,
            cc_phy_range_max,
            cc_val,
            formula: None,
        })
    }
}

fn read_link(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let link = LittleEndian::read_u64(&bytes[*offset..*offset + 8]);
    *offset += 8;
    if link == 0 { None } else { Some(link) }
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, MdfError> {
    if bytes.len() < *offset + 8 {
        return Err(MdfError::TooShortBuffer {
            actual: bytes.len(),
            expected: *offset + 8,
            file: file!(),
            line: line!(),
        });
    }
    let val = LittleEndian::read_u64(&bytes[*offset..*offset + 8]);
    *offset += 8;
    Ok(val)
}

impl ConversionBlock {
    /// Serialize this conversion block back to bytes.
    ///
    /// # Returns
    /// A byte vector containing the encoded block or an [`MdfError`] if
    /// serialization fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MdfError> {
        let links = 4 + self.cc_ref.len();

        let mut header = self.header.clone();
        header.links_nr = links as u64;

        let mut size = 24 + links * 8 + 1 + 1 + 2 + 2 + 2;
        if self.cc_flags & 0b10 != 0 {
            size += 16;
        }
        size += self.cc_val.len() * 8;
        header.block_len = size as u64;

        let mut buf = Vec::with_capacity(size);
        buf.extend_from_slice(&header.to_bytes()?);
        for link in [self.cc_tx_name, self.cc_md_unit, self.cc_md_comment, self.cc_cc_inverse] {
            buf.extend_from_slice(&link.unwrap_or(0).to_le_bytes());
        }
        for l in &self.cc_ref {
            buf.extend_from_slice(&l.to_le_bytes());
        }
        buf.push(self.cc_type.to_u8());
        buf.push(self.cc_precision);
        buf.extend_from_slice(&self.cc_flags.to_le_bytes());
        buf.extend_from_slice(&(self.cc_ref_count).to_le_bytes());
        buf.extend_from_slice(&(self.cc_val_count).to_le_bytes());
        if self.cc_flags & 0b10 != 0 {
            buf.extend_from_slice(&self.cc_phy_range_min.unwrap_or(0.0).to_le_bytes());
            buf.extend_from_slice(&self.cc_phy_range_max.unwrap_or(0.0).to_le_bytes());
        }
        for v in &self.cc_val {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        if buf.len() != size {
            return Err(MdfError::BlockSerializationError(format!(
                "ConversionBlock expected size {size} but wrote {}",
                buf.len()
            )));
        }
        Ok(buf)
    }
}
