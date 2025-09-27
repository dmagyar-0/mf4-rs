use crate::blocks::conversion::base::ConversionBlock;
use crate::blocks::common::{BlockHeader, read_string_block, BlockParse};
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;
use super::linear::extract_numeric;

/// Given `cc_val = [min0, max0, min1, max1, …]`, return the first index where
/// `raw` falls into `[min_i, max_i]`.
/// If no range matches, returns `n` (the default index).
pub fn find_range_to_text_index(cc_val: &[f64], raw: f64, inclusive_upper: bool) -> usize {
    let len = cc_val.len();
    if len < 2 || len % 2 != 0 { return 0; }
    let n = len / 2;
    for i in 0..n {
        let min = cc_val[2*i];
        let max = cc_val[2*i + 1];
        if inclusive_upper {
            if raw >= min && raw <= max { return i; }
        } else {
            if raw >= min && raw <  max { return i; }
        }
    }
    n
}

pub fn apply_value_to_text(block: &ConversionBlock, value: DecodedValue, file_data: &[u8]) -> Result<DecodedValue, MdfError> {
    let raw = match extract_numeric(&value) { Some(x) => x, None => return Ok(value) };
    let idx = block.cc_val.iter().position(|&k| k == raw).unwrap_or(block.cc_val.len());
    let link = *block.cc_ref.get(idx).unwrap_or(&0);
    if link == 0 { return Ok(DecodedValue::Unknown); }
    let off = link as usize;
    if off + 24 > file_data.len() { return Ok(DecodedValue::Unknown); }
    let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;
    if hdr.id == "##TX" {
        if let Some(txt) = read_string_block(file_data, link)? {
            return Ok(DecodedValue::String(txt));
        }
        return Ok(DecodedValue::Unknown);
    }
    if hdr.id == "##CC" {
        let mut nested = ConversionBlock::from_bytes(&file_data[off..])?;
        let _ = nested.resolve_formula(file_data);
        return nested.apply_decoded(value, file_data);
    }
    Ok(DecodedValue::Unknown)
}

pub fn apply_range_to_text(block: &ConversionBlock, value: DecodedValue, file_data: &[u8]) -> Result<DecodedValue, MdfError> {
    let raw = match extract_numeric(&value) { Some(x) => x, None => return Ok(value) };
    let inclusive_upper = matches!(value, DecodedValue::UnsignedInteger(_) | DecodedValue::SignedInteger(_));
    let idx = find_range_to_text_index(&block.cc_val, raw, inclusive_upper);
    let link = *block.cc_ref.get(idx).unwrap_or(&0);
    if link == 0 { return Ok(DecodedValue::Unknown); }
    let off = link as usize;
    if off + 24 > file_data.len() { return Ok(DecodedValue::Unknown); }
    let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;
    if hdr.id == "##TX" {
        return match read_string_block(file_data, link)? {
            Some(txt) => Ok(DecodedValue::String(txt)),
            None => Ok(DecodedValue::Unknown),
        };
    }
    if hdr.id == "##CC" {
        let mut nested = ConversionBlock::from_bytes(&file_data[off..])?;
        let _ = nested.resolve_formula(file_data);
        return nested.apply_decoded(value, file_data);
    }
    Ok(DecodedValue::Unknown)
}

pub fn apply_text_to_value(block: &ConversionBlock, value: DecodedValue, file_data: &[u8]) -> Result<DecodedValue, MdfError> {
    let input = match value { DecodedValue::String(s) => s, other => return Ok(other) };
    let n = block.cc_ref.len();
    for i in 0..n {
        let link = block.cc_ref[i];
        if link == 0 { continue; }
        if let Some(key_str) = read_string_block(file_data, link)? {
            if input == key_str {
                if i < block.cc_val.len() {
                    return Ok(DecodedValue::Float(block.cc_val[i]));
                } else {
                    return Ok(DecodedValue::Unknown);
                }
            }
        }
    }
    if block.cc_val.len() > n {
        Ok(DecodedValue::Float(block.cc_val[n]))
    } else {
        Ok(DecodedValue::Unknown)
    }
}

pub fn apply_text_to_text(block: &ConversionBlock, value: DecodedValue, file_data: &[u8]) -> Result<DecodedValue, MdfError> {
    let input = match value { DecodedValue::String(s) => s, other => return Ok(other) };
    let pairs = block.cc_ref.len().saturating_sub(1) / 2;
    for i in 0..pairs {
        let key_link = block.cc_ref[2*i];
        let output_link = block.cc_ref[2*i + 1];
        if let Some(key_str) = read_string_block(file_data, key_link)? {
            if key_str == input {
                return if output_link == 0 {
                    Ok(DecodedValue::String(input))
                } else {
                    Ok(read_string_block(file_data, output_link)?.map(DecodedValue::String).unwrap_or(DecodedValue::String(input)))
                };
            }
        }
    }
    let default_link = *block.cc_ref.get(2*pairs).unwrap_or(&0);
    if default_link == 0 {
        Ok(DecodedValue::String(input))
    } else {
        Ok(read_string_block(file_data, default_link)?.map(DecodedValue::String).unwrap_or(DecodedValue::String(input)))
    }
}
