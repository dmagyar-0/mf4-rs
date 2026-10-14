//! Single-pass, multi-channel numeric decoder.
//!
//! Reading channels one at a time walks the whole record stream once per
//! channel; for wide records (hundreds of signals per record) that is one
//! full sweep of the file per signal. This module decodes any number of
//! channels of one channel group in a **single sweep**:
//!
//! * the record stream is processed in cache-sized tiles of whole records;
//! * for each tile, every requested channel is extracted with a kernel chosen
//!   **once** per channel (no per-sample `match` on data type);
//! * linear conversions and invalidation masks are applied as separate tight
//!   passes over the tile's output, which the compiler can vectorise;
//! * row ranges are split across threads (native targets) writing to disjoint
//!   output slices — no locking, no merge step.
//!
//! Semantics match [`crate::api::channel::Channel::values_as_f64`]: physical
//! values, invalid samples and non-numeric conversion results are `NaN`.

use byteorder::{BigEndian, ByteOrder, LittleEndian};

use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::common::DataType;
use crate::blocks::conversion::ConversionType;
use crate::error::MdfError;
use crate::parsing::compression::worker_threads;
use crate::parsing::decoder::{decode_channel_value, decode_f64_from_record, DecodedValue};
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;

/// Target bytes of record data per tile (fits comfortably in L2).
const TILE_BYTES: usize = 256 * 1024;
/// Below this many output values the thread spawn overhead dominates.
const PARALLEL_MIN_VALUES: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kernel {
    F64Le(usize),
    F32Le(usize),
    F64Be(usize),
    F32Be(usize),
    U8(usize),
    I8(usize),
    U16Le(usize),
    I16Le(usize),
    U32Le(usize),
    I32Le(usize),
    U64Le(usize),
    I64Le(usize),
    U16Be(usize),
    I16Be(usize),
    U32Be(usize),
    I32Be(usize),
    U64Be(usize),
    I64Be(usize),
    /// Bit field packed in at most 8 bytes, little endian.
    BitsLe { off: usize, nbytes: usize, shift: u32, mask: u64, signed: bool },
    /// Bit field packed in at most 8 bytes, big endian.
    BitsBe { off: usize, nbytes: usize, shift: u32, mask: u64, signed: bool },
    /// Everything else: fall back to the reference per-record decoder.
    Generic,
    /// Statically undecodable (bit_count 0, bit_offset > 7, out of record).
    Nan,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Conv {
    None,
    Linear(f64, f64),
    /// Non-linear conversion: decode + convert per record via the reference path.
    Generic,
}

struct Column<'a> {
    block: &'a ChannelBlock,
    kernel: Kernel,
    conv: Conv,
    /// (byte index within record, bit mask) of the invalidation bit.
    inval: Option<(usize, u8)>,
    all_invalid: bool,
}

fn select_kernel(ch: &ChannelBlock, record_id_len: usize, record_size: usize) -> Kernel {
    let bit_count = ch.bit_count as usize;
    let bit_offset = ch.bit_offset as usize;
    if bit_count == 0 || bit_offset > 7 {
        return Kernel::Nan;
    }
    let off = record_id_len + ch.byte_offset as usize;
    let nbytes = (bit_offset + bit_count).div_ceil(8).max(1);
    if off + nbytes > record_size {
        return Kernel::Nan;
    }
    use DataType::*;
    let aligned = bit_offset == 0;
    match (&ch.data_type, aligned, bit_count) {
        (FloatLE, true, 64) => Kernel::F64Le(off),
        (FloatLE, true, 32) => Kernel::F32Le(off),
        (FloatBE, true, 64) => Kernel::F64Be(off),
        (FloatBE, true, 32) => Kernel::F32Be(off),
        (UnsignedIntegerLE | UnsignedIntegerBE, true, 8) => Kernel::U8(off),
        (SignedIntegerLE | SignedIntegerBE, true, 8) => Kernel::I8(off),
        (UnsignedIntegerLE, true, 16) => Kernel::U16Le(off),
        (SignedIntegerLE, true, 16) => Kernel::I16Le(off),
        (UnsignedIntegerLE, true, 32) => Kernel::U32Le(off),
        (SignedIntegerLE, true, 32) => Kernel::I32Le(off),
        (UnsignedIntegerLE, true, 64) => Kernel::U64Le(off),
        (SignedIntegerLE, true, 64) => Kernel::I64Le(off),
        (UnsignedIntegerBE, true, 16) => Kernel::U16Be(off),
        (SignedIntegerBE, true, 16) => Kernel::I16Be(off),
        (UnsignedIntegerBE, true, 32) => Kernel::U32Be(off),
        (SignedIntegerBE, true, 32) => Kernel::I32Be(off),
        (UnsignedIntegerBE, true, 64) => Kernel::U64Be(off),
        (SignedIntegerBE, true, 64) => Kernel::I64Be(off),
        (UnsignedIntegerLE | SignedIntegerLE | UnsignedIntegerBE | SignedIntegerBE, _, _)
            if nbytes <= 8 && bit_count < 64 =>
        {
            let signed = matches!(ch.data_type, SignedIntegerLE | SignedIntegerBE);
            let mask = (1u64 << bit_count) - 1;
            let shift = bit_offset as u32;
            if matches!(ch.data_type, UnsignedIntegerLE | SignedIntegerLE) {
                Kernel::BitsLe { off, nbytes, shift, mask, signed }
            } else {
                Kernel::BitsBe { off, nbytes, shift, mask, signed }
            }
        }
        _ => Kernel::Generic,
    }
}

impl<'a> Column<'a> {
    fn new(ch: &'a ChannelBlock, record_id_len: usize, data_bytes: usize, inval_bytes: usize) -> Self {
        let record_size = record_id_len + data_bytes + inval_bytes;
        let conv = match ch.conversion.as_ref() {
            None => Conv::None,
            Some(c) if c.cc_type == ConversionType::Identity => Conv::None,
            Some(c) if c.cc_type == ConversionType::Linear && c.cc_val.len() >= 2 => {
                Conv::Linear(c.cc_val[0], c.cc_val[1])
            }
            Some(_) => Conv::Generic,
        };
        // Mirrors decoder::check_value_validity.
        let all_invalid = ch.flags & 0x01 != 0;
        let inval = if inval_bytes > 0 && !all_invalid && ch.flags & 0x02 != 0 {
            let byte = record_id_len + data_bytes + (ch.pos_invalidation_bit >> 3) as usize;
            let bit = (ch.pos_invalidation_bit & 0x07) as u8;
            (byte < record_size).then_some((byte, 1u8 << bit))
        } else {
            None
        };
        let kernel = select_kernel(ch, record_id_len, record_size);
        Column { block: ch, kernel, conv, inval, all_invalid }
    }
}

macro_rules! strided {
    ($rows:expr, $rs:expr, $out:expr, $off:expr, |$s:ident| $read:expr) => {{
        for (o, rec) in $out.iter_mut().zip($rows.chunks_exact($rs)) {
            let $s = &rec[$off..];
            *o = $read;
        }
    }};
}

#[inline(always)]
fn bits_to_f64(raw: u64, shift: u32, mask: u64, signed: bool) -> f64 {
    let v = (raw >> shift) & mask;
    if signed {
        let sign = (mask >> 1) + 1;
        if v & sign != 0 { (v | !mask) as i64 as f64 } else { v as f64 }
    } else {
        v as f64
    }
}

/// Decode one tile (`rows` = whole records) for one column into `out`.
fn decode_tile(
    col: &Column<'_>,
    rows: &[u8],
    rs: usize,
    record_id_len: usize,
    mmap: &[u8],
    out: &mut [f64],
) -> Result<(), MdfError> {
    if col.all_invalid {
        out.fill(f64::NAN);
        return Ok(());
    }
    if col.conv == Conv::Generic {
        for (o, rec) in out.iter_mut().zip(rows.chunks_exact(rs)) {
            *o = match decode_channel_value(rec, record_id_len, col.block) {
                Some(d) => match col.block.apply_conversion_value(d, mmap)? {
                    DecodedValue::Float(v) => v,
                    DecodedValue::UnsignedInteger(v) => v as f64,
                    DecodedValue::SignedInteger(v) => v as f64,
                    _ => f64::NAN,
                },
                None => f64::NAN,
            };
        }
    } else {
        match col.kernel {
            Kernel::F64Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_f64(s)),
            Kernel::F32Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_f32(s) as f64),
            Kernel::F64Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_f64(s)),
            Kernel::F32Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_f32(s) as f64),
            Kernel::U8(off) => strided!(rows, rs, out, off, |s| s[0] as f64),
            Kernel::I8(off) => strided!(rows, rs, out, off, |s| s[0] as i8 as f64),
            Kernel::U16Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_u16(s) as f64),
            Kernel::I16Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_i16(s) as f64),
            Kernel::U32Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_u32(s) as f64),
            Kernel::I32Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_i32(s) as f64),
            Kernel::U64Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_u64(s) as f64),
            Kernel::I64Le(off) => strided!(rows, rs, out, off, |s| LittleEndian::read_i64(s) as f64),
            Kernel::U16Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_u16(s) as f64),
            Kernel::I16Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_i16(s) as f64),
            Kernel::U32Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_u32(s) as f64),
            Kernel::I32Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_i32(s) as f64),
            Kernel::U64Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_u64(s) as f64),
            Kernel::I64Be(off) => strided!(rows, rs, out, off, |s| BigEndian::read_i64(s) as f64),
            Kernel::BitsLe { off, nbytes, shift, mask, signed } => {
                strided!(rows, rs, out, off, |s| {
                    let mut b = [0u8; 8];
                    b[..nbytes].copy_from_slice(&s[..nbytes]);
                    bits_to_f64(u64::from_le_bytes(b), shift, mask, signed)
                })
            }
            Kernel::BitsBe { off, nbytes, shift, mask, signed } => {
                strided!(rows, rs, out, off, |s| {
                    let mut b = [0u8; 8];
                    b[8 - nbytes..].copy_from_slice(&s[..nbytes]);
                    bits_to_f64(u64::from_be_bytes(b), shift, mask, signed)
                })
            }
            Kernel::Generic => {
                for (o, rec) in out.iter_mut().zip(rows.chunks_exact(rs)) {
                    *o = decode_f64_from_record(rec, record_id_len, col.block);
                }
            }
            Kernel::Nan => out.fill(f64::NAN),
        }
        if let Conv::Linear(a, b) = col.conv {
            for v in out.iter_mut() {
                *v = a + b * *v;
            }
        }
    }
    if let Some((byte, mask)) = col.inval {
        for (o, rec) in out.iter_mut().zip(rows.chunks_exact(rs)) {
            if rec[byte] & mask != 0 {
                *o = f64::NAN;
            }
        }
    }
    Ok(())
}

/// A contiguous run of whole records and its first global row index.
struct Run<'a> {
    data: &'a [u8],
    first_row: usize,
    rows: usize,
}

/// Decode a row range `[r0, r1)` of every column into `outs` (each slice
/// covers exactly that range).
#[allow(clippy::too_many_arguments)]
fn decode_range(
    cols: &[Column<'_>],
    runs: &[Run<'_>],
    rs: usize,
    record_id_len: usize,
    mmap: &[u8],
    r0: usize,
    r1: usize,
    outs: &mut [&mut [f64]],
) -> Result<(), MdfError> {
    let tile_rows = (TILE_BYTES / rs).max(16);
    for run in runs {
        let run_end = run.first_row + run.rows;
        let lo = r0.max(run.first_row);
        let hi = r1.min(run_end);
        let mut row = lo;
        while row < hi {
            let n = tile_rows.min(hi - row);
            let local = row - run.first_row;
            let rows = &run.data[local * rs..(local + n) * rs];
            let o0 = row - r0;
            for (col, out) in cols.iter().zip(outs.iter_mut()) {
                decode_tile(col, rows, rs, record_id_len, mmap, &mut out[o0..o0 + n])?;
            }
            row += n;
        }
    }
    Ok(())
}

/// Decode several fixed-length numeric channels of one channel group in a
/// single sweep over the record data.
///
/// VLSD channels are not supported here (their samples do not live in the
/// fixed record); callers route those through the record iterator.
pub fn decode_columns_f64(
    dg: &RawDataGroup,
    cg: &RawChannelGroup,
    channels: &[&ChannelBlock],
    mmap: &[u8],
) -> Result<Vec<Vec<f64>>, MdfError> {
    if let Some(ch) = channels.iter().find(|c| c.channel_type == 1 && c.data != 0) {
        return Err(MdfError::BlockSerializationError(format!(
            "decode_columns_f64: VLSD channel (byte_offset {}) must use the record iterator",
            ch.byte_offset
        )));
    }
    let record_id_len = dg.block.record_id_len as usize;
    let data_bytes = cg.block.samples_byte_nr as usize;
    let inval_bytes = cg.block.invalidation_bytes_nr as usize;
    let rs = record_id_len + data_bytes + inval_bytes;
    if rs == 0 || channels.is_empty() {
        return Ok(vec![Vec::new(); channels.len()]);
    }

    let blocks = dg.data_blocks(mmap)?;
    let mut runs = Vec::with_capacity(blocks.len());
    let mut total = 0usize;
    for b in &blocks {
        let rows = b.data.len() / rs;
        if rows > 0 {
            runs.push(Run { data: &b.data[..rows * rs], first_row: total, rows });
            total += rows;
        }
    }

    let cols: Vec<Column<'_>> = channels
        .iter()
        .map(|c| Column::new(c, record_id_len, data_bytes, inval_bytes))
        .collect();
    let mut outs: Vec<Vec<f64>> = (0..cols.len()).map(|_| vec![0.0f64; total]).collect();

    let threads = if total.saturating_mul(cols.len()) >= PARALLEL_MIN_VALUES {
        worker_threads().min(total / 1024).max(1)
    } else {
        1
    };

    if threads <= 1 {
        let mut slices: Vec<&mut [f64]> = outs.iter_mut().map(|v| v.as_mut_slice()).collect();
        decode_range(&cols, &runs, rs, record_id_len, mmap, 0, total, &mut slices)?;
        return Ok(outs);
    }

    // Split each output column at the same row boundaries and hand each
    // thread its disjoint set of slices.
    let chunk = total.div_ceil(threads);
    let bounds: Vec<(usize, usize)> = (0..threads)
        .map(|t| (t * chunk, ((t + 1) * chunk).min(total)))
        .filter(|(a, b)| a < b)
        .collect();
    let mut per_thread: Vec<Vec<&mut [f64]>> = (0..bounds.len()).map(|_| Vec::new()).collect();
    for out in outs.iter_mut() {
        let mut rest: &mut [f64] = out.as_mut_slice();
        for (t, (a, b)) in bounds.iter().enumerate() {
            let (head, tail) = rest.split_at_mut(b - a);
            per_thread[t].push(head);
            rest = tail;
        }
    }
    let result: Result<(), MdfError> = std::thread::scope(|s| {
        let handles: Vec<_> = per_thread
            .into_iter()
            .zip(&bounds)
            .map(|(mut slices, &(a, b))| {
                let cols = &cols;
                let runs = &runs;
                s.spawn(move || decode_range(cols, runs, rs, record_id_len, mmap, a, b, &mut slices))
            })
            .collect();
        handles.into_iter().try_for_each(|h| {
            h.join().unwrap_or_else(|_| {
                Err(MdfError::BlockSerializationError("decode worker panicked".into()))
            })
        })
    });
    result?;
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::decoder::check_value_validity;

    fn chan(dt: DataType, byte_offset: u32, bit_offset: u8, bit_count: u32) -> ChannelBlock {
        let mut c = ChannelBlock::default();
        c.data_type = dt;
        c.byte_offset = byte_offset;
        c.bit_offset = bit_offset;
        c.bit_count = bit_count;
        c
    }

    /// Every kernel must agree bit-for-bit with the reference decoder.
    #[test]
    fn kernels_match_reference_decoder() {
        use DataType::*;
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let rid = 1usize;
        let data_bytes = 24usize;
        let rs = rid + data_bytes + 1;
        let rows: Vec<u8> = (0..rs * 257).map(|_| rnd() as u8).collect();
        let types = [UnsignedIntegerLE, UnsignedIntegerBE, SignedIntegerLE, SignedIntegerBE, FloatLE, FloatBE];
        let mut checked = 0;
        for dt in types {
            for bit_count in [1u32, 3, 7, 8, 12, 16, 17, 31, 32, 33, 48, 63, 64] {
                for bit_offset in [0u8, 1, 5, 7] {
                    for byte_offset in [0u32, 3, 15] {
                        let ch = chan(dt.clone(), byte_offset, bit_offset, bit_count);
                        let col = Column::new(&ch, rid, data_bytes, 1);
                        let mut out = vec![0.0; 257];
                        decode_tile(&col, &rows, rs, rid, &[], &mut out).unwrap();
                        for (i, rec) in rows.chunks_exact(rs).enumerate() {
                            let r = decode_f64_from_record(rec, rid, &ch);
                            assert!(
                                r.to_bits() == out[i].to_bits() || (r.is_nan() && out[i].is_nan()),
                                "{dt:?} bc={bit_count} bo={bit_offset} off={byte_offset} row {i}: ref {r} got {}",
                                out[i]
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 900);
    }

    #[test]
    fn invalidation_matches_reference() {
        let rid = 0;
        let rs = 8 + 1;
        let rows: Vec<u8> = (0..rs * 64).map(|i| (i * 37 % 256) as u8).collect();
        let mut ch = chan(DataType::UnsignedIntegerLE, 0, 0, 32);
        ch.flags = 0x02;
        ch.pos_invalidation_bit = 3;
        let col = Column::new(&ch, rid, 8, 1);
        let mut out = vec![0.0; 64];
        decode_tile(&col, &rows, rs, rid, &[], &mut out).unwrap();
        for (i, rec) in rows.chunks_exact(rs).enumerate() {
            assert_eq!(out[i].is_nan(), !check_value_validity(rec, rid, 8, &ch), "row {i}");
        }
    }
}
