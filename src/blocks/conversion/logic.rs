use crate::blocks::conversion::base::ConversionBlock;
use crate::blocks::conversion::types::ConversionType;
use crate::blocks::common::{BlockHeader, read_string_block, BlockParse};
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;

use meval::{Context, eval_str_with_context};

impl ConversionBlock {
    /// Helper: Attempts to extract a numeric value (as f64) from a DecodedValue.
    /// Returns Some(f64) if the input is numeric, or None otherwise.
    fn extract_numeric(value: &DecodedValue) -> Option<f64> {
        match value {
            DecodedValue::Float(n) => Some(*n),
            DecodedValue::UnsignedInteger(n) => Some(*n as f64),
            DecodedValue::SignedInteger(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// General table lookup: either interpolated or nearest‑neighbor.
    ///
    /// cc_val must be [key0, val0, key1, val1, …].  
    /// If interp==true → linear interpolation;  
    /// else nearest‑neighbor (tie → lower).
    fn lookup_table(cc_val: &[f64], raw: f64, interp: bool) -> Option<f64> {
        let len = cc_val.len();
        if len < 4 || len % 2 != 0 { return None; }
        let n = len / 2;
        // build (key, val) pairs
        let mut table = Vec::with_capacity(n);
        for i in 0..n {
            table.push((cc_val[2*i], cc_val[2*i + 1]));
        }
        // clamp below/above
        if raw <= table[0].0 {
            return Some(table[0].1);
        }
        if raw >= table[n-1].0 {
            return Some(table[n-1].1);
        }
        // find the segment
        for i in 0..(n-1) {
            let (k0, v0) = table[i];
            let (k1, v1) = table[i+1];
            if raw >= k0 && raw <= k1 {
                if interp {
                    // interpolation
                    let t = (raw - k0) / (k1 - k0);
                    return Some(v0 + t * (v1 - v0));
                } else {
                    // nearest‑neighbor
                    let d0 = raw - k0;
                    let d1 = k1 - raw;
                    return Some(if d1 < d0 { v1 } else { v0 });
                }
            }
        }
        None
    }

    /// Given cc_val = [min0, max0, min1, max1, …], returns:
    /// - the first i where raw ∈ [min_i..=max_i] (inclusive for integers, exclusive upper for floats)
    /// - or n (the default index) if none match
    fn find_range_to_text_index(cc_val: &[f64], raw: f64, inclusive_upper: bool) -> usize {
        let len = cc_val.len();
        if len < 2 || len % 2 != 0 {
            return 0; // malformed; will pick link[0] or default below
        }
        let n = len / 2;
        for i in 0..n {
            let min = cc_val[2*i];
            let max = cc_val[2*i + 1];
            if inclusive_upper {
                if raw >= min && raw <= max {
                    return i;
                }
            } else {
                if raw >= min && raw <  max {
                    return i;
                }
            }
        }
        n
    }

    /// Applies the conversion formula to a decoded channel value.
    ///
    /// Depending on the conversion type, this method either returns a numeric value
    /// (wrapped as DecodedValue::Float) or a character string (wrapped as DecodedValue::String).
    /// For non-numeric conversions such as Algebraic or Table look-ups, placeholder implementations
    /// are provided and can be extended later.
    ///
    /// # Parameters
    /// * `value`: The already-decoded channel value (as DecodedValue).
    ///
    /// # Returns
    /// A DecodedValue where numeric conversion types yield a Float and string conversion types yield a String.
    pub fn apply_decoded(
        &self,
        value: DecodedValue,
        file_data: &[u8],
    ) -> Result<DecodedValue, MdfError> {
        
        match self.cc_type {
            ConversionType::Identity => Ok(value),
            ConversionType::Linear => {
                if let Some(raw) = Self::extract_numeric(&value) {
                    if self.cc_val.len() >= 2 {
                        let result = self.cc_val[0] + self.cc_val[1] * raw;
                        Ok(DecodedValue::Float(result))
                    } else {
                        Ok(DecodedValue::Float(raw))
                    }
                } else {
                    Ok(value)
                }
            },
            ConversionType::Rational => {
                if let Some(raw) = Self::extract_numeric(&value) {
                    // Need exactly 6 parameters
                    if self.cc_val.len() >= 6 {
                        let p1 = self.cc_val[0];
                        let p2 = self.cc_val[1];
                        let p3 = self.cc_val[2];
                        let p4 = self.cc_val[3];
                        let p5 = self.cc_val[4];
                        let p6 = self.cc_val[5];
        
                        let num = p1 * raw * raw + p2 * raw + p3;
                        let den = p4 * raw * raw + p5 * raw + p6;
                        if den.abs() > std::f64::EPSILON {
                            Ok(DecodedValue::Float(num / den))
                        } else {
                            // Avoid division by zero; return raw or some sentinel
                            Ok(DecodedValue::Float(raw))
                        }
                    } else {
                        // Not enough parameters: fall back to raw
                        Ok(DecodedValue::Float(raw))
                    }
                } else {
                    // Non-numeric input
                    Ok(value)
                }
            },
            ConversionType::Algebraic => {
               // Ensure we have both a numeric input and a formula string.
                if let (Some(raw), Some(expr_str)) =
                (Self::extract_numeric(&value), self.formula.as_ref())
            {
                let mut ctx = Context::new();
                ctx.var("X", raw);
                match eval_str_with_context(expr_str, ctx)
                {
                    Ok(res) => Ok(DecodedValue::Float(res)),
                    Err(_)  => Ok(DecodedValue::Float(raw)), // fallback on error
                }
            } else {
                // Missing raw or missing formula: leave as-is
                Ok(value)
            }
            },
            ConversionType::TableLookupInterp => {
                if let Some(raw) = Self::extract_numeric(&value) {
                    let phys = Self::lookup_table(&self.cc_val, raw, true)
                        .unwrap_or(raw);
                    Ok(DecodedValue::Float(phys))
                } else {
                    Ok(value)
                }
            },
            ConversionType::TableLookupNoInterp => {
                if let Some(raw) = Self::extract_numeric(&value) {
                    let phys = Self::lookup_table(&self.cc_val, raw, false)
                        .unwrap_or(raw);
                    Ok(DecodedValue::Float(phys))
                } else {
                    Ok(value)
                }
            },
            ConversionType::RangeLookup => {
                // 1) extract raw and determine inclusive upper for ints
                if let Some(raw) = Self::extract_numeric(&value) {
                    // Here we decide inclusive vs exclusive:
                    // If the original was Float, upper-exclusive; otherwise inclusive.
                    let inclusive_upper = matches!(value, 
                        DecodedValue::UnsignedInteger(_) | DecodedValue::SignedInteger(_)
                    );

                    // 2) Check we have (3*n + 1) entries
                    let v = &self.cc_val;
                    if v.len() < 4 || (v.len() - 1) % 3 != 0 {
                        // malformed table: fallback to raw
                        return Ok(DecodedValue::Float(raw));
                    }
                    let n = (v.len() - 1) / 3;

                    // 3) The default value is the last element
                    let default = v[3 * n];

                    // 4) Scan each range triple: [ key_min, key_max, phys ]
                    for i in 0..n {
                        let key_min = v[3*i];
                        let key_max = v[3*i + 1];
                        let phys    = v[3*i + 2];

                        if inclusive_upper {
                            // integer input: key_min ≤ raw ≤ key_max
                            if raw >= key_min && raw <= key_max {
                                return Ok(DecodedValue::Float(phys));
                            }
                        } else {
                            // floating input: key_min ≤ raw < key_max
                            if raw >= key_min && raw <  key_max {
                                return Ok(DecodedValue::Float(phys));
                            }
                        }
                    }

                    // 5) No range matched → default
                    Ok(DecodedValue::Float(default))
                } else {
                    Ok(value)
                }
            },
        

            ConversionType::ValueToText => {
               
                // 1) Only proceed for numeric inputs:
                let raw = match Self::extract_numeric(&value) {
                    Some(x) => x,
                    None    => return Ok(value),
                };

                // 2) Find exact match index or default
                let idx = {
                    let i = self.cc_val.iter()
                        .position(|&k| k == raw)
                        .unwrap_or(self.cc_val.len());
                    i
                };

                // 3) Get the corresponding link (n keys → n+1 links)
                let link = *self.cc_ref.get(idx).unwrap_or(&0);
                if link == 0 {
                    return Ok(DecodedValue::Unknown);
                }

                // 4) Read the block header at that link
                let off = link as usize;
                if off + 24 > file_data.len() {
                    return Ok(DecodedValue::Unknown);
                }
                let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;

                // 5a) TXBLOCK → return its text
                if hdr.id == "##TX" {
                    if let Some(txt) = read_string_block(file_data, link)? {
                        return Ok(DecodedValue::String(txt));
                    }
                    return Ok(DecodedValue::Unknown);
                }

                // 5b) CCBLOCK → nested scale conversion
                if hdr.id == "##CC" {
                    let mut nested =
                        ConversionBlock::from_bytes(&file_data[off..])?;
                    {
                        let _ = nested.resolve_formula(file_data);
                        return nested.apply_decoded(value, file_data);
                    }
                }

                // 6) Fallback
                Ok(DecodedValue::Unknown)
            },

            ConversionType::RangeToText => {
                // 1) extract numeric + int‑flag
                let raw = match Self::extract_numeric(&value) {
                    Some(x) => x,
                    None    => return Ok(value),   // pass through non‑numeric
                };
                let inclusive_upper = matches!(value,
                    DecodedValue::UnsignedInteger(_) | DecodedValue::SignedInteger(_)
                );

                // 2) find index in [0..n]  (n keys → n+1 links)
                let idx = Self::find_range_to_text_index(
                    &self.cc_val,
                    raw,
                    inclusive_upper,
                );

                // 3) pick the link
                let link = *self.cc_ref.get(idx).unwrap_or(&0);
                if link == 0 {
                    return Ok(DecodedValue::Unknown);
                }

                // 4) bounds‑check + read header
                let off = link as usize;
                if off + 24 > file_data.len() {
                    return Ok(DecodedValue::Unknown);
                }
                let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;

                // 5a) TXBLOCK → text
                if hdr.id == "##TX" {
                    return match read_string_block(file_data, link)? {
                        Some(txt) => Ok(DecodedValue::String(txt)),
                        None      => Ok(DecodedValue::Unknown),
                    };
                }

                // 5b) CCBLOCK → nested scale conversion
                if hdr.id == "##CC" {
                    let mut nested =
                        ConversionBlock::from_bytes(&file_data[off..])?;
                    {
                        let _ = nested.resolve_formula(file_data);
                        return nested.apply_decoded(value, file_data);
                    }
                }

                // 6) fallback
                Ok(DecodedValue::Unknown)
            },
            ConversionType::TextToValue => {
                // 1) Only handle String inputs
                let input = match value {
                    DecodedValue::String(s) => s,
                    other => return Ok(other),
                };

                // 2) Number of table entries is the number of cc_ref links
                //    (there must be exactly n links, and n+1 cc_val entries)
                let n = self.cc_ref.len();

                // 3) Iterate over each key link
                for i in 0..n {
                    let link = self.cc_ref[i];
                    if link == 0 {
                        // spec says key links must not be NIL, but just skip if they are
                        continue;
                    }
                    // Safely read the TextBlock at that link
                    if let Some(key_str) = read_string_block(file_data, link)? {
                        // case‑sensitive comparison
                        if input == key_str {
                            // match → return cc_val[i]
                            if i < self.cc_val.len() {
                                return Ok(DecodedValue::Float(self.cc_val[i]));
                            } else {
                                return Ok(DecodedValue::Unknown);
                            }
                        }
                    }
                }

                // 4) No match → default value is cc_val[n]
                if self.cc_val.len() > n {
                    Ok(DecodedValue::Float(self.cc_val[n]))
                } else {
                    // malformed table
                    Ok(DecodedValue::Unknown)
                }
            },
            ConversionType::TextToText => {
                // Only handle string inputs
                let input = match value {
                    DecodedValue::String(s) => s,
                    other => return Ok(other),
                };

                // Number of key‐output pairs
                let pairs = self.cc_ref.len().saturating_sub(1) / 2;
                // Iterate (0..pairs): cc_ref[2*i] is key‑link, cc_ref[2*i+1] is output‑link
                for i in 0..pairs {
                    let key_link    = self.cc_ref[2*i];
                    let output_link = self.cc_ref[2*i + 1];
                    // Read the key string (must not be NIL)
                    if let Some(key_str) = read_string_block(file_data, key_link)? {
                        if key_str == input {
                            // Match!  If output_link is NIL, return input; else read & return that TXBLOCK text
                            return if output_link == 0 {
                                Ok(DecodedValue::String(input))
                            } else {
                                Ok(read_string_block(file_data, output_link)?
                                    .map(DecodedValue::String)
                                    .unwrap_or(DecodedValue::String(input)))
                            };
                        }
                    }
                }

                // No key matched → default link at cc_ref[2*pairs]
                let default_link = *self.cc_ref.get(2*pairs).unwrap_or(&0);
                if default_link == 0 {
                    // NIL default: identity
                    Ok(DecodedValue::String(input))
                } else {
                    // read default TXBLOCK (if malformed, fall back to input)
                    Ok(read_string_block(file_data, default_link)?
                        .map(DecodedValue::String)
                        .unwrap_or(DecodedValue::String(input)))
                }
            },
            ConversionType::BitfieldText => {
                // 1) Only proceed for integer inputs:
                let raw = match value {
                    DecodedValue::UnsignedInteger(u) => u as u64,
                    DecodedValue::SignedInteger(i)   => i as u64,
                    _ => return Ok(value),
                };

                let mut parts = Vec::new();
                let masks = &self.cc_val;     // f64 stash of UINT64 bitmasks
                let links = &self.cc_ref;     // same length n

                // 2) For each table entry
                for (i, &link_addr) in links.iter().enumerate() {
                    // out‑of‑bounds safety
                    if i >= masks.len() { break; }

                    // mask is stored as a REAL; reinterpret its bits as u64
                    let mask = masks[i].to_bits();
                    let masked = raw & mask;

                    // if no link => skip
                    if link_addr == 0 {
                        continue;
                    }

                    let off = link_addr as usize;
                    if off + 24 > file_data.len() {
                        continue;
                    }

                    // must be a CCBLOCK
                    let hdr = BlockHeader::from_bytes(&file_data[off..off+24])?;
                    if &hdr.id != "##CC" {
                        continue;
                    }

                    // parse the nested conversion block
                    let mut nested =
                        ConversionBlock::from_bytes(&file_data[off..])?;
                    {
                        // resolve its formula if needed
                        let _ = nested.resolve_formula(file_data);
                        // apply it to the masked integer
                        let decoded_masked =
                            nested.apply_decoded(
                                DecodedValue::UnsignedInteger(masked),
                                file_data,
                            )?;
                        // if it yields a string, we include it
                        if let DecodedValue::String(s) = decoded_masked {
                            // if the nested CCBLOCK has a name, prefix it
                            let part = if let Some(name_ptr) = nested.header.links_nr.checked_sub(0).and_then(|_| nested.header.links_nr.checked_sub(0)).and_then(|_| {
                                // actually read CC_TX_NAME: it's the first fixed link in the nested header
                                // which you stored in nested.cc_tx_name
                                nested.cc_tx_name
                            }) {
                                if let Some(name) = read_string_block(file_data, name_ptr)? {
                                    format!("{} = {}", name, s)
                                } else {
                                    s
                                }
                            } else {
                                s
                            };
                            parts.push(part);
                        }
                    }
                }

                // 3) join with '|'
                let out = parts.join("|");
                Ok(DecodedValue::String(out))
            },
            ConversionType::Unknown(_) => Ok(value),
        }
    }

}