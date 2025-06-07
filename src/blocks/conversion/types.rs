/// Represents the conversion type (cc_type) from a conversion block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionType {
    /// 0: 1:1 conversion (no change)
    Identity,
    /// 1: Linear conversion
    Linear,
    /// 2: Rational conversion
    Rational,
    /// 3: Algebraic conversion (MCD-2 MC text formula)
    Algebraic,
    /// 4: Value to value tabular look-up with interpolation
    TableLookupInterp,
    /// 5: Value to value tabular look-up without interpolation
    TableLookupNoInterp,
    /// 6: Value range to value tabular look-up
    RangeLookup,
    /// 7: Value to text/scale conversion tabular look-up
    ValueToText,
    /// 8: Value range to text/scale conversion tabular look-up
    RangeToText,
    /// 9: Text to value tabular look-up
    TextToValue,
    /// 10: Text to text tabular look-up (translation)
    TextToText,
    /// 11: Bitfield text table
    BitfieldText,
    /// For any other unrecognized conversion type.
    Unknown(u8),
}

impl ConversionType {
    /// Converts a raw u8 value to the corresponding ConversionType.
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => ConversionType::Identity,
            1 => ConversionType::Linear,
            2 => ConversionType::Rational,
            3 => ConversionType::Algebraic,
            4 => ConversionType::TableLookupInterp,
            5 => ConversionType::TableLookupNoInterp,
            6 => ConversionType::RangeLookup,
            7 => ConversionType::ValueToText,
            8 => ConversionType::RangeToText,
            9 => ConversionType::TextToValue,
            10 => ConversionType::TextToText,
            11 => ConversionType::BitfieldText,
            other => ConversionType::Unknown(other),
        }
    }
}
