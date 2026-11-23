//! Reading a VLSD channel record by record must not hold one entry per
//! record in memory: `RawChannel::records()` is lazy, and `cut_mdf_by_time`
//! resolves each record's payload as it copies the record.
//!
//! A counting global allocator measures the peak heap growth of each call
//! (this test binary holds a single test, so nothing else allocates
//! concurrently).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::mdf_file::MdfFile;
use mf4_rs::writer::MdfWriter;
use mf4_rs::{DecodedValue, Mdf};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run `f` and return its result with the peak heap growth it caused.
fn peak_growth<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = CURRENT.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let result = f();
    (result, PEAK.load(Ordering::SeqCst) - base)
}

const RECORDS: usize = 300_000;

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join("mf4rs_vlsd_memory");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_str().unwrap().to_string()
}

/// `Time` + a UTF-8 VLSD `Msg` channel over `RECORDS` records.
fn vlsd_file(path: &str) {
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group_with(None, |_| {}).unwrap();
    let t = w
        .add_channel_with(&cg, None, |c| {
            c.data_type = DataType::FloatLE;
            c.bit_count = 64;
            c.name = Some("Time".into());
        })
        .unwrap();
    w.set_time_channel(&t).unwrap();
    let vlsd = w
        .add_channel_with(&cg, Some(&t), |c| {
            c.data_type = DataType::StringUtf8;
            c.bit_count = 64;
            c.channel_type = 1;
            c.data = 1;
            c.name = Some("Msg".into());
        })
        .unwrap();
    w.start_data_block_for_cg_raw(&cg, 0, 16, 0).unwrap();
    w.start_signal_data_block(&vlsd).unwrap();
    let mut offset = 0u64;
    for i in 0..RECORDS {
        let payload = format!("m{}", i % 10);
        let mut r = Vec::with_capacity(16);
        r.extend_from_slice(&(i as f64).to_le_bytes());
        r.extend_from_slice(&offset.to_le_bytes());
        w.write_raw_record(&cg, &r).unwrap();
        w.write_signal_data(&vlsd, payload.as_bytes()).unwrap();
        offset += 4 + payload.len() as u64;
    }
    w.finish_signal_data_block(&vlsd).unwrap();
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();
}

#[test]
fn vlsd_record_reads_do_not_hold_every_record_in_memory() -> Result<(), MdfError> {
    let path = tmp("vlsd_memory.mf4");
    vlsd_file(&path);
    // One `Result<&[u8], MdfError>` per record would be ~56 bytes each.
    let per_record_list = RECORDS * 40;

    let file = MdfFile::parse_from_file(&path)?;
    let dg = &file.data_groups[0];
    let cg = &dg.channel_groups[0];
    let (first, growth) = peak_growth(|| {
        let mut records = cg.raw_channels[1].records(dg, cg, &file.mmap).unwrap();
        records.next().map(|r| r.map(<[u8]>::to_vec))
    });
    assert_eq!(first.unwrap()?, b"m0".to_vec());
    assert!(
        growth < per_record_list / 10,
        "records() + next() grew the heap by {growth} bytes for {RECORDS} records"
    );

    let (count, growth) = peak_growth(|| cg.raw_channels[1].records(dg, cg, &file.mmap).unwrap().count());
    assert_eq!(count, RECORDS);
    assert!(growth < per_record_list / 10, "iterating records() grew the heap by {growth} bytes");

    let out = tmp("vlsd_memory_cut.mf4");
    let (cut, growth) = peak_growth(|| mf4_rs::cut_mdf_by_time(&path, &out, 0.0, RECORDS as f64));
    cut?;
    assert!(
        growth < per_record_list / 2,
        "cut_mdf_by_time grew the heap by {growth} bytes for {RECORDS} records"
    );
    let values = Mdf::from_file(&out)?.read("Msg", None)?.values;
    assert_eq!(values.len(), RECORDS);
    assert_eq!(values[RECORDS - 1], Some(DecodedValue::String(format!("m{}", (RECORDS - 1) % 10))));
    Ok(())
}
