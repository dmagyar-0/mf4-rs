use crate::blocks::conversion::base::ConversionBlock;
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;
use super::linear::extract_numeric;

/// General table lookup: either interpolated or nearest neighbour.
/// `cc_val` must be `[key0, val0, key1, val1, …]` with ascending keys (as
/// the MDF spec requires). Inputs below / above the table clamp to its first
/// / last value; `NaN` has no result.
///
/// Binary search over the keys in place: no allocation, `O(log n)` per value.
pub fn lookup_table(cc_val: &[f64], raw: f64, interp: bool) -> Option<f64> {
    let len = cc_val.len();
    // A single key/value pair is spec-valid: every input clamps to that value.
    if len < 2 || !len.is_multiple_of(2) { return None; }
    let n = len / 2;
    let key = |i: usize| cc_val[2 * i];
    let val = |i: usize| cc_val[2 * i + 1];
    if raw <= key(0) { return Some(val(0)); }
    if raw >= key(n - 1) { return Some(val(n - 1)); }
    if raw.is_nan() { return None; }
    // First segment [k_i, k_i+1] holding `raw`: the first i whose upper key
    // k_i+1 >= raw (key(0) < raw < key(n-1) here, so 0 <= i < n-1).
    let (mut lo, mut hi) = (1, n - 1);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if key(mid) < raw { lo = mid + 1 } else { hi = mid }
    }
    let i = lo - 1;
    let (k0, v0, k1, v1) = (key(i), val(i), key(i + 1), val(i + 1));
    if interp {
        let t = (raw - k0) / (k1 - k0);
        Some(v0 + t * (v1 - v0))
    } else {
        let d0 = raw - k0;
        let d1 = k1 - raw;
        Some(if d1 < d0 { v1 } else { v0 })
    }
}

pub fn apply_table_lookup(block: &ConversionBlock, value: DecodedValue, interp: bool) -> Result<DecodedValue, MdfError> {
    if let Some(raw) = extract_numeric(&value) {
        let phys = lookup_table(&block.cc_val, raw, interp).unwrap_or(raw);
        Ok(DecodedValue::Float(phys))
    } else {
        Ok(value)
    }
}

pub fn apply_range_lookup(block: &ConversionBlock, value: DecodedValue) -> Result<DecodedValue, MdfError> {
    if let Some(raw) = extract_numeric(&value) {
        let inclusive_upper = matches!(value, DecodedValue::UnsignedInteger(_) | DecodedValue::SignedInteger(_));
        let v = &block.cc_val;
        if v.len() < 4 || !(v.len() - 1).is_multiple_of(3) {
            return Ok(DecodedValue::Float(raw));
        }
        let n = (v.len() - 1) / 3;
        let default = v[3 * n];
        for i in 0..n {
            let key_min = v[3*i];
            let key_max = v[3*i + 1];
            let phys    = v[3*i + 2];
            if inclusive_upper {
                if raw >= key_min && raw <= key_max { return Ok(DecodedValue::Float(phys)); }
            } else {
                if raw >= key_min && raw <  key_max { return Ok(DecodedValue::Float(phys)); }
            }
        }
        Ok(DecodedValue::Float(default))
    } else {
        Ok(value)
    }
}
