use byteorder::{LittleEndian, ByteOrder};
use crate::blocks::common::{BlockHeader, BlockParse};
use crate::error::MdfError;
use super::types::ConversionType;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    
    // Resolved data for self-contained conversions (populated during index creation)
    /// Pre-resolved text strings for text-based conversions (ValueToText, RangeToText, etc.)
    /// Maps cc_ref indices to their resolved text content
    pub resolved_texts: Option<std::collections::HashMap<usize, String>>,
    
    /// Pre-resolved nested conversion blocks for chained conversions
    /// Maps cc_ref indices to their resolved ConversionBlock content
    pub resolved_conversions: Option<std::collections::HashMap<usize, Box<ConversionBlock>>>,
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
            resolved_texts: None,
            resolved_conversions: None,
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
    /// Resolve all dependencies for this conversion block to make it self-contained.
    /// This reads referenced text blocks and nested conversions from the file data
    /// and stores them in the resolved_texts and resolved_conversions fields.
    ///
    /// # Arguments
    /// * `file_data` - Memory mapped MDF bytes used to read referenced data
    ///
    /// # Returns
    /// `Ok(())` on success or an [`MdfError`] if resolution fails
    pub fn resolve_all_dependencies(&mut self, file_data: &[u8]) -> Result<(), MdfError> {
        use crate::blocks::common::{read_string_block, BlockHeader};
        use std::collections::HashMap;
        
        // First resolve the formula if this is an algebraic conversion
        self.resolve_formula(file_data)?;
        
        // Initialize resolved data containers
        let mut resolved_texts = HashMap::new();
        let mut resolved_conversions = HashMap::new();
        
        // Resolve each reference in cc_ref
        for (i, &link_addr) in self.cc_ref.iter().enumerate() {
            if link_addr == 0 {
                continue; // Skip null links
            }
            
            let offset = link_addr as usize;
            if offset + 24 > file_data.len() {
                continue; // Skip invalid offsets
            }
            
            // Read the block header to determine the type
            let header = BlockHeader::from_bytes(&file_data[offset..offset + 24])?;
            
            match header.id.as_str() {
                "##TX" => {
                    // Text block - resolve the string content
                    if let Some(text) = read_string_block(file_data, link_addr)? {
                        resolved_texts.insert(i, text);
                    }
                }
                "##CC" => {
                    // Nested conversion block - resolve recursively
                    let mut nested_conversion = ConversionBlock::from_bytes(&file_data[offset..])?;
                    nested_conversion.resolve_all_dependencies(file_data)?;
                    resolved_conversions.insert(i, Box::new(nested_conversion));
                }
                _ => {
                    // Other block types - ignore for now
                }
            }
        }
        
        // Store resolved data if any was found
        if !resolved_texts.is_empty() {
            self.resolved_texts = Some(resolved_texts);
        }
        if !resolved_conversions.is_empty() {
            self.resolved_conversions = Some(resolved_conversions);
        }
        
        Ok(())
    }
    
    /// Get a resolved text string for a given cc_ref index.
    /// Returns the text if it was resolved during dependency resolution.
    pub fn get_resolved_text(&self, ref_index: usize) -> Option<&String> {
        self.resolved_texts.as_ref()?.get(&ref_index)
    }
    
    /// Get a resolved nested conversion for a given cc_ref index.
    /// Returns the conversion block if it was resolved during dependency resolution.
    pub fn get_resolved_conversion(&self, ref_index: usize) -> Option<&ConversionBlock> {
        self.resolved_conversions.as_ref()?.get(&ref_index).map(|boxed| boxed.as_ref())
    }

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
