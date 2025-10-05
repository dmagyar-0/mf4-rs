use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::common::DataType;
use byteorder::{LittleEndian, BigEndian, ByteOrder};

// Flag bit positions for cn_flags
const CN_FLAG_ALL_INVALID: u32 = 0x01;  // Bit 0: All values are invalid
const CN_FLAG_INVAL_BIT_VALID: u32 = 0x02;  // Bit 1: Invalidation bit is valid

/// An enum representing the decoded value of a channel sample.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedValue {
    UnsignedInteger(u64),
    SignedInteger(i64),
    Float(f64),
    String(String),
    ByteArray(Vec<u8>),
    MimeSample(Vec<u8>),
    MimeStream(Vec<u8>),
    Unknown,
}

/// Result of decoding a channel value, including validity status.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedChannelValue {
    pub value: DecodedValue,
    pub is_valid: bool,
}

/// Checks if a channel value is valid based on invalidation bits.
///
/// According to MDF 4.1 spec section 4.21.5.1:
/// - If cn_flags bit 0 is set (1), all values are invalid
/// - If cn_flags bits 0 and 1 are both clear (0), all values are valid
/// - Otherwise, must check the invalidation bit in the record
///
/// # Parameters
/// - `record`: The complete record bytes including record ID, data, and invalidation bytes
/// - `record_id_size`: Number of bytes for the record ID
/// - `cg_data_bytes`: Number of bytes for the data portion (samples_byte_nr from channel group)
/// - `channel`: The channel block containing flags and invalidation bit position
///
/// # Returns
/// `true` if the value is valid, `false` if invalid
pub fn check_value_validity(
    record: &[u8],
    record_id_size: usize,
    cg_data_bytes: u32,
    channel: &ChannelBlock,
) -> bool {
    // Check cn_flags first for shortcuts
    if channel.flags & CN_FLAG_ALL_INVALID != 0 {
        // Bit 0 set: all values are invalid
        return false;
    }
    
    if channel.flags & (CN_FLAG_ALL_INVALID | CN_FLAG_INVAL_BIT_VALID) == 0 {
        // Bits 0 and 1 both clear: all values are valid
        return true;
    }
    
    // Must check the invalidation bit in the record
    // Location: record_id + data_bytes + (cn_inval_bit_pos >> 3)
    let inval_byte_offset = record_id_size + cg_data_bytes as usize 
                          + (channel.pos_invalidation_bit >> 3) as usize;
    let inval_bit_index = (channel.pos_invalidation_bit & 0x07) as usize;
    
    if inval_byte_offset < record.len() {
        let inval_byte = record[inval_byte_offset];
        let bit_is_set = (inval_byte >> inval_bit_index) & 0x01 != 0;
        // If the invalidation bit is set (1), the value is INVALID
        !bit_is_set
    } else {
        // No invalidation byte available, assume valid
        true
    }
}

/// Decodes a channel's sample from a record (legacy function without validity checking).
///
/// This function takes the raw record data, skips over the record ID,
/// and then uses channel metadata (offsets, bit settings, and data type)
/// from the given `ChannelBlock` to decode the sample. It supports numeric
/// types (unsigned/signed integers, floats), strings (Latin1, UTF-8, UTF-16LE,
/// UTF-16BE), byte arrays, and MIME samples/streams.
/// 
/// # Parameters
/// - `record`: A slice containing the entire record's bytes.
/// - `record_id_size`: The number of bytes reserved at the beginning of the record for the record ID.
/// - `channel`: A reference to the channel metadata used for decoding.
/// 
/// # Returns
/// An `Option<DecodedValue>` containing the decoded sample, or `None` if there isn't enough data.
/// 
/// # Note
/// This function does NOT check invalidation bits. For full MDF spec compliance,
/// use `decode_channel_value_with_validity` instead.
pub fn decode_channel_value(
    record: &[u8],
    record_id_size: usize,
    channel: &ChannelBlock,
) -> Option<DecodedValue> {
    decode_value_internal(record, record_id_size, channel)
}

/// Decodes a channel's sample from a record with validity checking.
///
/// This function performs the full MDF 4.1 spec-compliant decoding including
/// invalidation bit checking. It returns both the decoded value and whether
/// the value is valid according to the invalidation bits.
/// 
/// # Parameters
/// - `record`: A slice containing the entire record's bytes (including invalidation bytes)
/// - `record_id_size`: The number of bytes reserved at the beginning of the record for the record ID
/// - `cg_data_bytes`: Number of data bytes in the record (samples_byte_nr from channel group)
/// - `channel`: A reference to the channel metadata used for decoding
/// 
/// # Returns
/// An `Option<DecodedChannelValue>` containing the decoded sample and validity status,
/// or `None` if there isn't enough data to decode.
pub fn decode_channel_value_with_validity(
    record: &[u8],
    record_id_size: usize,
    cg_data_bytes: u32,
    channel: &ChannelBlock,
) -> Option<DecodedChannelValue> {
    let value = decode_value_internal(record, record_id_size, channel)?;
    let is_valid = check_value_validity(record, record_id_size, cg_data_bytes, channel);
    
    Some(DecodedChannelValue { value, is_valid })
}

/// Internal function that performs the actual value decoding.
///
/// This is the core decoding logic separated out so it can be used by both
/// the legacy function and the new validity-aware function.
fn decode_value_internal(
    record: &[u8],
    record_id_size: usize,
    channel: &ChannelBlock,
) -> Option<DecodedValue> {
    
    // Calculate the starting offset of this channel's data.
    let base_offset = record_id_size + channel.byte_offset as usize;
    let bit_offset = channel.bit_offset as usize;
    let bit_count = channel.bit_count as usize;

    let slice: &[u8] = if channel.channel_type == 1 && channel.data != 0 {
        // VLSD: the entire record *is* the payload
        record
    } else {
        // For non-numeric types, assume the field is stored in whole bytes.
        let num_bytes = if matches!(channel.data_type,
            DataType::StringLatin1 | DataType::StringUtf8 | DataType::StringUtf16LE | DataType::StringUtf16BE |
            DataType::ByteArray | DataType::MimeSample | DataType::MimeStream)
        {
            bit_count / 8
        } else {
            ((bit_offset + bit_count + 7) / 8).max(1)
        };

        if base_offset + num_bytes > record.len() {
            return None;
        }
        &record[base_offset..base_offset + num_bytes]
    };

    match &channel.data_type {
        DataType::UnsignedIntegerLE => {
            let raw = slice.iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            let shifted = raw >> bit_offset;
            let mask = if bit_count >= 64 { u64::MAX } else { (1u64 << bit_count) - 1 };
            Some(DecodedValue::UnsignedInteger(shifted & mask))
        },
        DataType::UnsignedIntegerBE => {
            let raw = slice.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            let shifted = raw >> bit_offset;
            let mask = if bit_count >= 64 { u64::MAX } else { (1u64 << bit_count) - 1 };
            Some(DecodedValue::UnsignedInteger(shifted & mask))
        },
        DataType::SignedIntegerLE => {
            let raw = slice.iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            let shifted = raw >> bit_offset;
            let mask = if bit_count >= 64 { u64::MAX } else { (1u64 << bit_count) - 1 };
            let unsigned = shifted & mask;
            let sign_bit = 1u64 << (bit_count - 1);
            let signed = if unsigned & sign_bit != 0 {
                (unsigned as i64) | (!(mask as i64))
            } else {
                unsigned as i64
            };
            Some(DecodedValue::SignedInteger(signed))
        },
        DataType::SignedIntegerBE => {
            let raw = slice.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            let shifted = raw >> bit_offset;
            let mask = if bit_count >= 64 { u64::MAX } else { (1u64 << bit_count) - 1 };
            let unsigned = shifted & mask;
            let sign_bit = 1u64 << (bit_count - 1);
            let signed = if unsigned & sign_bit != 0 {
                (unsigned as i64) | (!(mask as i64))
            } else {
                unsigned as i64
            };
            Some(DecodedValue::SignedInteger(signed))
        },
        DataType::FloatLE => {
            let raw = slice.iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            if bit_count == 32 {
                Some(DecodedValue::Float(f32::from_bits(raw as u32) as f64))
            } else if bit_count == 64 {
                Some(DecodedValue::Float(f64::from_bits(raw)))
            } else {
                None
            }
        },
        DataType::FloatBE => {
            let raw = slice.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
            if bit_count == 32 {
                Some(DecodedValue::Float(f32::from_bits(raw as u32) as f64))
            } else if bit_count == 64 {
                Some(DecodedValue::Float(f64::from_bits(raw)))
            } else {
                None
            }
        },
        DataType::StringLatin1 => {
            // Latin1: each byte maps directly to a character.
            let s: String = slice.iter().map(|&b| b as char).collect();
            Some(DecodedValue::String(s.trim_end_matches('\0').to_string()))
        },
        DataType::StringUtf8 => {
            match std::str::from_utf8(slice) {
                Ok(s) => Some(DecodedValue::String(s.trim_end_matches('\0').to_string())),
                Err(_) => Some(DecodedValue::String(String::from("<Invalid UTF8>")))
            }
        },
        DataType::StringUtf16LE => {
            if slice.len() % 2 != 0 { return None; }
            let u16_data: Vec<u16> = slice.chunks_exact(2)
                .map(|chunk| LittleEndian::read_u16(chunk))
                .collect();
            match String::from_utf16(&u16_data) {
                Ok(s) => Some(DecodedValue::String(s.trim_end_matches('\0').to_string())),
                Err(_) => Some(DecodedValue::String(String::from("<Invalid UTF16LE>")))
            }
        },
        DataType::StringUtf16BE => {
            if slice.len() % 2 != 0 { return None; }
            let u16_data: Vec<u16> = slice.chunks_exact(2)
                .map(|chunk| BigEndian::read_u16(chunk))
                .collect();
            match String::from_utf16(&u16_data) {
                Ok(s) => Some(DecodedValue::String(s.trim_end_matches('\0').to_string())),
                Err(_) => Some(DecodedValue::String(String::from("<Invalid UTF16BE>")))
            }
        },
        DataType::ByteArray => Some(DecodedValue::ByteArray(slice.to_vec())),
        DataType::MimeSample => Some(DecodedValue::MimeSample(slice.to_vec())),
        DataType::MimeStream => Some(DecodedValue::MimeStream(slice.to_vec())),
        _ => Some(DecodedValue::Unknown),
    }
}
