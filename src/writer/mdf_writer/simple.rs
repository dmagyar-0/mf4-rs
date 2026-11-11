//! The name-based writer API of the cross-language contract.
//!
//! These methods are what the Python and TypeScript `MdfWriter` classes
//! expose one-to-one (see `docs/API.md`). They sit on top of the
//! closure-based `*_with` building blocks, which remain available for code
//! that needs full control over the written blocks.

use super::MdfWriter;
use crate::blocks::common::DataType;
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;

/// A record value as a dynamically typed binding (Python, JavaScript) hands
/// it over, before it is matched against its channel's data type.
pub(crate) enum PlainValue<'a> {
    /// An integer (`int`, `bigint`).
    Int(i128),
    /// A floating-point number (`float`, `number`).
    Float(f64),
    Text(&'a str),
    Bytes(&'a [u8]),
    /// A boolean — never a valid record value.
    Bool,
    /// Anything else — never a valid record value.
    Other,
}

/// Turn a binding's plain value into the [`DecodedValue`] a channel of
/// `data_type` encodes. The one place the coercion rules live, so Python and
/// TypeScript accept and refuse exactly the same values:
///
/// * float channels take any number;
/// * integer channels take integers, and numbers with an integral value, that
///   are in range — never fractions, `NaN` or (for unsigned) negatives;
/// * text channels take text, `ByteArray` channels take bytes;
/// * booleans are refused.
pub(crate) fn record_value(value: PlainValue<'_>, data_type: &DataType, index: usize) -> Result<DecodedValue, MdfError> {
    let refuse = |expected: &str| {
        MdfError::InvalidArgument(format!(
            "record value at index {index} must be {expected} for a {} channel",
            data_type.name()
        ))
    };
    match data_type {
        DataType::FloatLE | DataType::FloatBE => match value {
            PlainValue::Float(f) => Ok(DecodedValue::Float(f)),
            PlainValue::Int(i) => Ok(DecodedValue::Float(i as f64)),
            _ => Err(refuse("a number")),
        },
        DataType::UnsignedIntegerLE | DataType::UnsignedIntegerBE => {
            let expected = "a non-negative integer";
            match value {
                PlainValue::Int(i) => u64::try_from(i).map(DecodedValue::UnsignedInteger).map_err(|_| refuse(expected)),
                PlainValue::Float(f) if f.fract() == 0.0 && (0.0..18446744073709551616.0).contains(&f) => {
                    Ok(DecodedValue::UnsignedInteger(f as u64))
                }
                _ => Err(refuse(expected)),
            }
        }
        DataType::SignedIntegerLE | DataType::SignedIntegerBE => {
            let expected = "an integer";
            match value {
                PlainValue::Int(i) => i64::try_from(i).map(DecodedValue::SignedInteger).map_err(|_| refuse(expected)),
                PlainValue::Float(f) if f.fract() == 0.0 && (-9223372036854775808.0..9223372036854775808.0).contains(&f) => {
                    Ok(DecodedValue::SignedInteger(f as i64))
                }
                _ => Err(refuse(expected)),
            }
        }
        dt if dt.is_string() => match value {
            PlainValue::Text(s) => Ok(DecodedValue::String(s.to_string())),
            _ => Err(refuse("a string")),
        },
        _ => match value {
            PlainValue::Bytes(b) => Ok(DecodedValue::ByteArray(b.to_vec())),
            _ => Err(refuse("bytes")),
        },
    }
}

impl MdfWriter {
    /// Set the measurement start time (nanoseconds since the Unix epoch, UTC).
    ///
    /// `init_mdf_file` stamps the header with the current time; call this
    /// afterwards to anchor the file to a specific timestamp. See
    /// [`set_start_time_with`](Self::set_start_time_with) for time-zone fields.
    pub fn set_start_time(&mut self, abs_time_ns: u64) -> Result<(), MdfError> {
        self.set_start_time_with(abs_time_ns, 0, 0, 0, 0)
    }

    /// Add a channel group (in its own data group) and return its id.
    pub fn add_channel_group(&mut self, name: Option<&str>) -> Result<String, MdfError> {
        let cg_id = self.add_channel_group_with(None, |_cg| {})?;
        if let Some(name) = name {
            self.set_channel_group_name(&cg_id, name)?;
        }
        Ok(cg_id)
    }

    /// Add a channel of `data_type` and return its id.
    ///
    /// Numeric types get their default width — **32 bits** for integers and
    /// floats (use `add_float_channel` / `add_int_channel` / `add_sint_channel`
    /// for 64-bit channels); a value that does not fit is refused by
    /// `write_record`. Text types and `ByteArray` are written as
    /// variable-length channels, text in the type's own encoding.
    ///
    /// Channels are laid out in the order they are added; `write_record`
    /// expects one value per channel in that order.
    pub fn add_channel(&mut self, group_id: &str, name: &str, data_type: DataType) -> Result<String, MdfError> {
        if data_type.is_string() || data_type == DataType::ByteArray {
            return self.add_vlsd_channel(group_id, name, data_type);
        }
        let bits = data_type.default_bits();
        self.add_fixed_channel(group_id, name, data_type, bits)
    }

    /// Add a 64-bit float channel and mark it as the group's master (time)
    /// channel. Add it first, before the group's data channels.
    pub fn add_time_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        let id = self.add_fixed_channel(group_id, name, DataType::FloatLE, 64)?;
        self.set_time_channel(&id)?;
        Ok(id)
    }

    /// Add a 64-bit little-endian float (`f64`) channel.
    pub fn add_float_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        self.add_fixed_channel(group_id, name, DataType::FloatLE, 64)
    }

    /// Add a 32-bit little-endian float (`f32`) channel.
    pub fn add_float32_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        self.add_fixed_channel(group_id, name, DataType::FloatLE, 32)
    }

    /// Add a 64-bit little-endian **unsigned** integer (`u64`) channel.
    pub fn add_int_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        self.add_fixed_channel(group_id, name, DataType::UnsignedIntegerLE, 64)
    }

    /// Add a 64-bit little-endian **signed** integer (`i64`) channel.
    pub fn add_sint_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        self.add_fixed_channel(group_id, name, DataType::SignedIntegerLE, 64)
    }

    /// Add a variable-length UTF-8 string channel.
    pub fn add_string_channel(&mut self, group_id: &str, name: &str) -> Result<String, MdfError> {
        self.add_vlsd_channel(group_id, name, DataType::StringUtf8)
    }

    /// Attach a linear conversion (`physical = offset + factor * raw`) to a
    /// channel. Call before `start_data_block`.
    pub fn add_linear_conversion(&mut self, channel_id: &str, offset: f64, factor: f64) -> Result<(), MdfError> {
        self.write_linear_conversion(offset, factor, Some(channel_id))?;
        Ok(())
    }

    /// Attach a value-to-text conversion to a channel: each `(value, text)`
    /// pair maps a raw integer to a label, anything else to `default_text`.
    /// Call before `start_data_block`.
    pub fn add_value_to_text_conversion(
        &mut self,
        channel_id: &str,
        mapping: &[(i64, &str)],
        default_text: &str,
    ) -> Result<(), MdfError> {
        self.write_value_to_text_conversion(mapping, default_text, Some(channel_id))?;
        Ok(())
    }

    /// Open the data block of a channel group. Call after all of the group's
    /// channels (and conversions) are added, before writing records.
    pub fn start_data_block(&mut self, group_id: &str) -> Result<(), MdfError> {
        self.start_data_block_for_cg(group_id, 0)
    }

    /// Data types of a group's channels, in record order.
    pub fn channel_data_types(&self, group_id: &str) -> Result<Vec<DataType>, MdfError> {
        self.cg_channels
            .get(group_id)
            .map(|chs| chs.iter().map(|ch| ch.data_type.clone()).collect())
            .ok_or_else(|| MdfError::InvalidArgument(format!("unknown channel group '{group_id}'")))
    }

    /// Id of the channel most recently added to a group (the link target for
    /// the next one).
    fn last_channel(&self, group_id: &str) -> Result<Option<String>, MdfError> {
        self.cg_channel_ids
            .get(group_id)
            .map(|ids| ids.last().cloned())
            .ok_or_else(|| MdfError::InvalidArgument(format!("unknown channel group '{group_id}'")))
    }

    fn add_fixed_channel(
        &mut self,
        group_id: &str,
        name: &str,
        data_type: DataType,
        bit_count: u32,
    ) -> Result<String, MdfError> {
        let prev = self.last_channel(group_id)?;
        self.add_channel_with(group_id, prev.as_deref(), |ch| {
            ch.data_type = data_type;
            ch.name = Some(name.to_string());
            ch.bit_count = bit_count;
        })
    }

    fn add_vlsd_channel(&mut self, group_id: &str, name: &str, data_type: DataType) -> Result<String, MdfError> {
        let prev = self.last_channel(group_id)?;
        self.add_channel_with(group_id, prev.as_deref(), |ch| {
            ch.data_type = data_type;
            ch.name = Some(name.to_string());
            ch.channel_type = 1; // VLSD
            ch.data = 1; // routed to the SD-block writer; patched on finish
            ch.bit_count = 64; // record slot holds a u64 offset
        })
    }
}
