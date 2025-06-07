use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::common::DataType;
use byteorder::{LittleEndian, BigEndian, ByteOrder};

/// An enum representing the decoded value of a channel sample.
#[derive(Debug)]
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

/// Decodes a channel's sample from a record.
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
/// An `Option<DecodedValue>` containing the decoded sample, or `None` if there isn’t enough data.
pub fn decode_channel_value(
    record: &[u8],
    record_id_size: usize,
    channel: &ChannelBlock,
) -> Option<DecodedValue> {
    
    // Calculate the starting offset of this channel's data.
    let base_offset = record_id_size + channel.byte_offset as usize;
    let bit_offset = channel.bit_offset as usize;
    let bit_count = channel.bit_count as usize;

    let slice: &[u8] = if channel.channel_type == 1 {
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
