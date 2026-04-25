use crate::error::MdfError;
use crate::parsing::mdf_file::MdfFile;
use crate::parsing::decoder::{decode_channel_value, DecodedValue};
use crate::writer::MdfWriter;

/// Cut a segment of an MDF file using **absolute** UNIX-epoch timestamps.
///
/// Unlike [`cut_mdf_by_time`], which interprets its bounds as seconds
/// relative to the master channel's zero, this entry point takes nanosecond
/// timestamps since the UNIX epoch and converts them to relative seconds
/// using the source file's `HD` block start time. This is convenient when
/// the caller already has wall-clock timestamps (e.g. parsed from an ISO
/// 8601 string or a `datetime` object).
///
/// Returns an error if the source file does not record an absolute start
/// time (its `HD.abs_time` is `0`).
///
/// # Arguments
/// * `input_path` - Path to the source MF4 file
/// * `output_path` - Destination path for the trimmed file
/// * `start_ns` - Start of the window in UNIX-epoch nanoseconds (inclusive)
/// * `end_ns` - End of the window in UNIX-epoch nanoseconds (inclusive)
pub fn cut_mdf_by_utc_ns(
    input_path: &str,
    output_path: &str,
    start_ns: i64,
    end_ns: i64,
) -> Result<(), MdfError> {
    // Peek at the source file just to read its absolute start time. This
    // mirrors the parse the main cut routine performs immediately after, but
    // we keep the two parses separate so the time math is self-contained.
    let mdf_for_anchor = MdfFile::parse_from_file(input_path)?;
    let file_start_ns: u64 = mdf_for_anchor.header.abs_time;
    if file_start_ns == 0 {
        return Err(MdfError::BlockSerializationError(
            "source file has no absolute start time (HD.abs_time = 0); \
             cannot cut by UTC timestamps"
                .into(),
        ));
    }
    drop(mdf_for_anchor);

    let anchor = file_start_ns as i128;
    let start_rel_s = (start_ns as i128 - anchor) as f64 / 1.0e9;
    let end_rel_s = (end_ns as i128 - anchor) as f64 / 1.0e9;

    cut_mdf_by_time(input_path, output_path, start_rel_s, end_rel_s)
}

/// Cut a segment of an MDF file based on time stamps.
///
/// The input file is scanned for a master time channel (channel type `2` and
/// sync type `1`). Records whose time value lies in the inclusive range
/// `[start_time, end_time]` are copied byte-for-byte to the output file,
/// preserving:
///
/// * fixed-length numeric, string, and byte-array channels,
/// * per-record invalidation bytes,
/// * VLSD ("signal-based") channels — fresh `##SD` blocks are written in the
///   output, one entry per kept record, and the channel's `data` link is
///   patched to point at them.
///
/// Per-channel conversion / source / metadata blocks are not re-emitted; the
/// output channels are written without conversions attached. The master
/// channel's conversion is still applied internally when filtering by time.
///
/// # Arguments
/// * `input_path` - Path to the source MF4 file
/// * `output_path` - Destination path for the trimmed file
/// * `start_time` - Start time of the segment in seconds (inclusive)
/// * `end_time` - End time of the segment in seconds (inclusive)
///
/// # Returns
/// `Ok(())` on success or an [`MdfError`] if reading or writing fails.
pub fn cut_mdf_by_time(
    input_path: &str,
    output_path: &str,
    start_time: f64,
    end_time: f64,
) -> Result<(), MdfError> {
    let mdf = MdfFile::parse_from_file(input_path)?;
    let mut writer = MdfWriter::new(output_path)?;
    writer.init_mdf_file()?;

    for dg in &mdf.data_groups {
        let record_id_len = dg.block.record_id_len;

        let mut prev_cg: Option<String> = None;
        for cg in &dg.channel_groups {
            let samples_byte_nr = cg.block.samples_byte_nr;
            let invalidation_bytes_nr = cg.block.invalidation_bytes_nr;
            let record_size = record_id_len as usize
                + samples_byte_nr as usize
                + invalidation_bytes_nr as usize;

            let cg_id = writer.add_channel_group(prev_cg.as_deref(), |_| {})?;
            prev_cg = Some(cg_id.clone());

            // Re-create channel blocks in the output. Stale link addresses
            // pointing into the source file are zeroed out so the resulting
            // channel block is self-contained. The VLSD `data` link is
            // patched later by `finish_signal_data_block`.
            let mut prev_cn: Option<String> = None;
            // (out_cn_id, source_channel_index, is_vlsd)
            let mut out_channels: Vec<(String, usize, bool)> = Vec::new();
            for (idx, ch) in cg.raw_channels.iter().enumerate() {
                let mut block = ch.block.clone();
                block.resolve_name(&mdf.mmap)?;

                let is_vlsd = block.channel_type == 1 && block.data != 0;

                // Drop links to source-file blocks we do not re-emit. Without
                // this, the new file would carry pointers into garbage.
                block.conversion_addr = 0;
                block.conversion = None;
                block.source_addr = 0;
                block.unit_addr = 0;
                block.comment_addr = 0;
                block.component_addr = 0;
                block.data = 0;

                let cn_id = writer.add_channel(&cg_id, prev_cn.as_deref(), |c| {
                    *c = block.clone();
                })?;
                prev_cn = Some(cn_id.clone());
                out_channels.push((cn_id, idx, is_vlsd));
            }

            // Open the output DT block using the source's exact record
            // layout (including invalidation bytes), and open one ##SD chain
            // per VLSD channel. We must remember the channel ids before the
            // mutable borrow of the writer ends.
            writer.start_data_block_for_cg_raw(
                &cg_id,
                record_id_len,
                samples_byte_nr,
                invalidation_bytes_nr,
            )?;

            let vlsd_out_ids: Vec<String> = out_channels
                .iter()
                .filter_map(|(cn_id, _, is_vlsd)| {
                    if *is_vlsd { Some(cn_id.clone()) } else { None }
                })
                .collect();
            for cn_id in &vlsd_out_ids {
                writer.start_signal_data_block(cn_id)?;
            }

            // Build VLSD source iterators (lockstep with parent records).
            let mut vlsd_iters: Vec<(String, Box<dyn Iterator<Item = Result<&[u8], MdfError>>>)> =
                Vec::new();
            for (cn_id, src_idx, is_vlsd) in &out_channels {
                if *is_vlsd {
                    let it = cg.raw_channels[*src_idx].records(dg, cg, &mdf.mmap)?;
                    vlsd_iters.push((cn_id.clone(), it));
                }
            }

            // Identify the master/time channel in the source CG.
            let time_idx = cg.raw_channels.iter().position(|c| {
                c.block.channel_type == 2 && c.block.sync_type == 1
            });

            // Iterate raw parent records from the source DT/DL chain.
            let blocks = dg.data_blocks(&mdf.mmap)?;
            'outer: for data_block in blocks {
                let raw = data_block.data;
                if record_size == 0 {
                    // Degenerate CG with no record bytes — nothing to do.
                    break;
                }
                let valid_len = (raw.len() / record_size) * record_size;
                for record_chunk in raw[..valid_len].chunks_exact(record_size) {
                    // Pull one VLSD entry per VLSD channel in lockstep with
                    // the parent record, regardless of whether we keep the
                    // record. This keeps the iterators aligned.
                    let mut vlsd_payloads: Vec<Vec<u8>> = Vec::with_capacity(vlsd_iters.len());
                    for (_, iter) in vlsd_iters.iter_mut() {
                        match iter.next() {
                            Some(Ok(slice)) => vlsd_payloads.push(slice.to_vec()),
                            Some(Err(e)) => return Err(e),
                            None => {
                                return Err(MdfError::BlockSerializationError(
                                    "VLSD entry count fewer than parent records".into(),
                                ));
                            }
                        }
                    }

                    // Decide whether this record falls in the time window.
                    let keep = if let Some(ti) = time_idx {
                        let ch = &cg.raw_channels[ti].block;
                        let raw_val = decode_channel_value(
                            record_chunk,
                            record_id_len as usize,
                            ch,
                        )
                        .unwrap_or(DecodedValue::Unknown);
                        let phys = if let Some(conv) = &ch.conversion {
                            conv.apply_decoded(raw_val, &mdf.mmap)?
                        } else {
                            raw_val
                        };
                        let t = match phys {
                            DecodedValue::Float(f) => f,
                            DecodedValue::UnsignedInteger(u) => u as f64,
                            DecodedValue::SignedInteger(i) => i as f64,
                            _ => continue,
                        };
                        if t < start_time {
                            false
                        } else if t - end_time > f64::EPSILON {
                            // Match the legacy epsilon comparison so floats
                            // produced by `i * 0.1` style timestamps remain
                            // inclusive of the upper bound.
                            break 'outer;
                        } else {
                            true
                        }
                    } else {
                        // No master channel — copy everything.
                        true
                    };

                    if keep {
                        writer.write_raw_record(&cg_id, record_chunk)?;
                        for ((cn_id, _), payload) in
                            vlsd_iters.iter().zip(vlsd_payloads.iter())
                        {
                            writer.write_signal_data(cn_id, payload)?;
                        }
                    }
                }
            }

            for cn_id in &vlsd_out_ids {
                writer.finish_signal_data_block(cn_id)?;
            }
            writer.finish_data_block(&cg_id)?;
        }
    }

    writer.finalize()
}
