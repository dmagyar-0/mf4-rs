use crate::blocks::conversion::base::ConversionBlock;
use crate::blocks::common::{BlockHeader, read_string_block, BlockParse};
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;

pub fn apply_bitfield_text(block: &ConversionBlock, value: DecodedValue, file_data: &[u8]) -> Result<DecodedValue, MdfError> {
    let raw = match value {
        DecodedValue::UnsignedInteger(u) => u as u64,
        DecodedValue::SignedInteger(i) => i as u64,
        _ => return Ok(value),
    };

    let mut parts = Vec::new();
    let masks = &block.cc_val;
    let links = &block.cc_ref;

    for (i, &link_addr) in links.iter().enumerate() {
        if i >= masks.len() { break; }
        let mask = masks[i].to_bits();
        let masked = raw & mask;
        if link_addr == 0 { continue; }
        
        // First try to use resolved conversions if available
        if let Some(resolved_conversion) = block.get_resolved_conversion(i) {
            let decoded_masked = resolved_conversion.apply_decoded(DecodedValue::UnsignedInteger(masked), &[])?;
            if let DecodedValue::String(s) = decoded_masked {
                // TODO: For bitfield conversions, we may need to store resolved names separately
                // For now, just use the string without the name prefix
                parts.push(s);
            }
            continue;
        }
        
        // Fallback to legacy behavior if no resolved data (for backward compatibility)
        let off = link_addr as usize;
        if off + 24 > file_data.len() { continue; }
        let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;
        if &hdr.id != "##CC" { continue; }
        let mut nested = ConversionBlock::from_bytes(&file_data[off..])?;
        let _ = nested.resolve_formula(file_data);
        let decoded_masked = nested.apply_decoded(DecodedValue::UnsignedInteger(masked), file_data)?;
        if let DecodedValue::String(s) = decoded_masked {
            let part = if let Some(name_ptr) = nested.cc_tx_name {
                if let Some(name) = read_string_block(file_data, name_ptr)? {
                    format!("{} = {}", name, s)
                } else {
                    s.clone()
                }
            } else {
                s.clone()
            };
            parts.push(part);
        }
    }

    let out = parts.join("|");
    Ok(DecodedValue::String(out))
}
