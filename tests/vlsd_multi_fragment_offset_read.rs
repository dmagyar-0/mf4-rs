//! Regression test for VLSD ##SD chains that span multiple fragments.
//!
//! When a VLSD payload exceeds `MAX_SD_BLOCK_SIZE` (4 MB), the writer splits
//! the buffered entries across several ##SD blocks linked via a ##DL block.
//! Because the splits land at entry boundaries (no entry is allowed to
//! straddle fragments), fragment sizes vary — in particular, an unusually
//! large entry can be alone in its own fragment, larger than its
//! predecessors.
//!
//! Earlier the writer emitted these DL blocks with the equal-length flag set
//! and the *first* fragment's size as the common length. Spec-conformant
//! random-access readers (asammdf, Vector) then use the equal-length value
//! to map an inline VLSD offset to a fragment, which goes wrong as soon as
//! actual fragment sizes diverge from that lie.
//!
//! mf4-rs's own reader walks SD entries sequentially, so this defect was
//! invisible to the existing test suite. This test simulates an offset-based
//! reader explicitly to lock in the fix.

use mf4_rs::api::mdf::MDF;
use mf4_rs::block_layout::FileLayout;
use mf4_rs::blocks::common::{BlockHeader, BlockParse, DataType};
use mf4_rs::blocks::data_list_block::DataListBlock;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

const RECORD_LEN: usize = 16; // 8 bytes time + 8 bytes VLSD slot

fn build_vlsd_with_uneven_fragments(path: &str) -> Result<Vec<Vec<u8>>, MdfError> {
    // Entry layout chosen to force the SD writer to produce >1 fragment with
    // *different* sizes: a small leading entry then a single ~5 MB entry.
    // The 5 MB entry exceeds the 4 MB-ish per-fragment cap on its own, so
    // the writer flushes the small leading entry as fragment 0, and emits
    // the big entry alone in fragment 1. Result: |F1| > |F0|.
    let payloads: Vec<Vec<u8>> = vec![
        vec![0xAA; 1 * 1024 * 1024], // 1 MB
        vec![0xBB; 5 * 1024 * 1024], // 5 MB (alone in its fragment)
        vec![0xCC; 256 * 1024],      // 256 KB (joins fragment 1 or starts F2)
    ];

    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    let vlsd = w.add_channel(&cg, Some(&t), |c| {
        c.data_type = DataType::ByteArray;
        c.bit_count = 64;
        c.channel_type = 1; // VLSD
        c.name = Some("Image".into());
    })?;
    w.start_data_block_for_cg_raw(&cg, 0, RECORD_LEN as u32, 0)?;
    w.start_signal_data_block(&vlsd)?;

    let mut running: u64 = 0;
    for (i, p) in payloads.iter().enumerate() {
        let mut record = Vec::with_capacity(RECORD_LEN);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&running.to_le_bytes());
        w.write_raw_record(&cg, &record)?;
        w.write_signal_data(&vlsd, p)?;
        running = running.checked_add(4 + p.len() as u64).unwrap();
    }
    w.finish_signal_data_block(&vlsd)?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    Ok(payloads)
}

/// Simulate a spec-conformant random-access VLSD reader (the way asammdf and
/// Vector resolve inline offsets to entries). Returns each entry's payload as
/// the offset-based reader would see it.
fn read_vlsd_via_offsets(
    bytes: &[u8],
    dl_addr: u64,
    inline_offsets: &[u64],
) -> Result<Vec<Vec<u8>>, MdfError> {
    let dl = DataListBlock::from_bytes(&bytes[dl_addr as usize..])?;
    assert!(dl.next == 0, "test assumes a single DL node");

    // Build (virtual_offset, data_section_slice) per fragment.
    let mut fragments: Vec<(u64, &[u8])> = Vec::with_capacity(dl.data_links.len());
    let virtual_starts: Vec<u64> = match (dl.flags & 1, &dl.data_block_len, &dl.offsets) {
        (1, Some(len), _) => {
            // Equal-length form: every fragment is asserted to be `len` bytes
            // including the 24-byte header.
            (0..dl.data_links.len())
                .map(|i| i as u64 * (len.saturating_sub(24)))
                .collect()
        }
        (0, _, Some(off)) => off.clone(),
        _ => panic!("malformed DL flags/payload combination"),
    };

    for (i, &addr) in dl.data_links.iter().enumerate() {
        let off = addr as usize;
        let header = BlockHeader::from_bytes(&bytes[off..off + 24])?;
        assert_eq!(header.id, "##SD", "expected SD fragment, got {}", header.id);
        let data = &bytes[off + 24..off + header.block_len as usize];
        fragments.push((virtual_starts[i], data));
    }

    let mut out = Vec::with_capacity(inline_offsets.len());
    for &virtual_offset in inline_offsets {
        // Locate the fragment whose virtual span contains the requested offset.
        let mut chosen: Option<(usize, u64)> = None;
        for (i, (vstart, data)) in fragments.iter().enumerate() {
            let vend = vstart + data.len() as u64;
            if virtual_offset >= *vstart && virtual_offset < vend {
                chosen = Some((i, virtual_offset - vstart));
                break;
            }
        }
        let (frag_idx, intra) = chosen.ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "virtual offset 0x{:x} not in any fragment",
                virtual_offset
            ))
        })?;
        let data = fragments[frag_idx].1;
        let intra = intra as usize;
        if intra + 4 > data.len() {
            return Err(MdfError::BlockSerializationError(format!(
                "fragment {} too short for length prefix at intra={}",
                frag_idx, intra
            )));
        }
        let len = u32::from_le_bytes(data[intra..intra + 4].try_into().unwrap()) as usize;
        if intra + 4 + len > data.len() {
            return Err(MdfError::BlockSerializationError(format!(
                "fragment {}: entry at intra={} length={} exceeds fragment size {}",
                frag_idx,
                intra,
                len,
                data.len()
            )));
        }
        out.push(data[intra + 4..intra + 4 + len].to_vec());
    }
    Ok(out)
}

#[test]
fn vlsd_multi_fragment_offset_read_round_trip() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("vlsd_multi_fragment_offset_read.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let payloads = build_vlsd_with_uneven_fragments(path.to_str().unwrap())?;

    // Sanity: mf4-rs's sequential reader still produces correct values.
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let read = chs[1].values()?;
    assert_eq!(read.len(), payloads.len());
    for (i, v) in read.iter().enumerate() {
        match v {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b.len(), payloads[i].len()),
            other => panic!("unexpected value at {}: {:?}", i, other),
        }
    }
    drop(mdf);

    // Locate the DL block via the file layout, then verify it uses the
    // variable-length form (flags=0). Equal-length DL is invalid for SD
    // chains with uneven fragment sizes — the whole point of the fix.
    let layout = FileLayout::from_file(path.to_str().unwrap())?;
    let dl_block = layout
        .blocks
        .iter()
        .find(|b| b.block_type == "##DL")
        .expect("expected a ##DL block linking the SD fragments");
    let dl_addr = dl_block.offset;

    let bytes = std::fs::read(&path)?;
    let dl_parsed = DataListBlock::from_bytes(&bytes[dl_addr as usize..])?;
    assert!(
        dl_parsed.flags & 1 == 0,
        "DL must use variable-length form (flags=0) for VLSD chains; got flags={:#x}",
        dl_parsed.flags
    );
    assert!(dl_parsed.offsets.is_some(), "variable-form DL must carry offsets");
    let offs = dl_parsed.offsets.as_ref().unwrap();
    assert_eq!(offs.len(), dl_parsed.data_links.len());
    assert_eq!(offs[0], 0, "first fragment must start at virtual offset 0");
    // Verify fragment sizes are actually uneven (otherwise this test isn't
    // exercising the bug it's supposed to lock down).
    let mut sizes = Vec::new();
    for &addr in &dl_parsed.data_links {
        let h = BlockHeader::from_bytes(&bytes[addr as usize..addr as usize + 24])?;
        sizes.push(h.block_len);
    }
    println!("SD fragments: count={} block_lens={:?}", sizes.len(), sizes);
    let all_equal = sizes.iter().all(|&s| s == sizes[0]);
    assert!(
        !all_equal,
        "test setup degenerate: all SD fragments have equal size; the bug \
         this test guards against only triggers when sizes differ"
    );
    assert!(sizes.len() >= 2, "expected the writer to produce >1 SD fragment");

    // Pull the inline VLSD offsets the writer wrote into each parent record.
    // Record layout: 8 bytes time + 8 bytes VLSD slot.
    let dt_block = layout
        .blocks
        .iter()
        .find(|b| b.block_type == "##DT")
        .expect("expected a ##DT block");
    let dt_off = dt_block.offset as usize;
    let dt_len = dt_block.size as usize;
    let dt_data = &bytes[dt_off + 24..dt_off + dt_len];
    let mut inline_offsets = Vec::new();
    for rec in dt_data.chunks_exact(RECORD_LEN) {
        let off = u64::from_le_bytes(rec[8..16].try_into().unwrap());
        inline_offsets.push(off);
    }
    assert_eq!(inline_offsets.len(), payloads.len());

    // Now do the offset-based read — Vector / asammdf semantics.
    let by_offset = read_vlsd_via_offsets(&bytes, dl_addr, &inline_offsets)?;
    assert_eq!(by_offset.len(), payloads.len());
    for (i, (got, want)) in by_offset.iter().zip(payloads.iter()).enumerate() {
        assert_eq!(
            got.len(),
            want.len(),
            "offset-based read of entry {} returned wrong length: got={} want={}",
            i,
            got.len(),
            want.len()
        );
        assert_eq!(
            got, want,
            "offset-based read of entry {} returned wrong bytes",
            i
        );
    }

    std::fs::remove_file(&path)?;
    Ok(())
}
