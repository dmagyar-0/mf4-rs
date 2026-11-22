//! A block chain that links back to itself must be refused with a "cycle"
//! error by every builder — before the fix, the range-reader walk (used by
//! `MdfIndex::from_url` and the wasm builder) and both index builders' `##DL`
//! walks looped until memory ran out, while `Mdf` already refused them.
//!
//! The range-reader tests use a reader that gives up after a fixed number of
//! reads, so a looping build fails the test instead of hanging it.

use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, MdfIndex, SliceRangeReader};
use mf4_rs::writer::{ColumnData, MdfWriter};
use mf4_rs::Mdf;

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join("mf4rs_cyclic_chains");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_str().unwrap().to_string()
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// Two-channel file (`Time`, `Val`) as bytes.
fn file_bytes() -> Vec<u8> {
    let path = tmp("source.mf4");
    let mut w = MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let g = w.add_channel_group(Some("G")).unwrap();
    w.add_time_channel(&g, "Time").unwrap();
    w.add_float_channel(&g, "Val").unwrap();
    w.start_data_block(&g).unwrap();
    let t: Vec<f64> = (0..10).map(|i| i as f64).collect();
    w.write_columns(&g, &[ColumnData::F64(&t), ColumnData::F64(&t)]).unwrap();
    w.finish_data_block(&g).unwrap();
    w.finalize().unwrap();
    std::fs::read(&path).unwrap()
}

struct Addrs {
    dg: usize,
    cg: usize,
    cn: usize,
}

fn addrs(b: &[u8]) -> Addrs {
    let dg = u64_at(b, 64 + 24) as usize;
    let cg = u64_at(b, dg + 32) as usize;
    let cn = u64_at(b, cg + 32) as usize;
    Addrs { dg, cg, cn }
}

/// Point the first data group at a `##DL` whose `next` link is itself.
fn with_cyclic_data_list(mut b: Vec<u8>) -> Vec<u8> {
    let a = addrs(&b);
    let dt = u64_at(&b, a.dg + 40);
    while b.len() % 8 != 0 {
        b.push(0);
    }
    let dl = b.len() as u64;
    b.extend_from_slice(b"##DL");
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&(24u64 + 16 + 8 + 8).to_le_bytes()); // 2 links + flags/count + 1 offset
    b.extend_from_slice(&2u64.to_le_bytes());
    b.extend_from_slice(&dl.to_le_bytes()); // dl_dl_next -> itself
    b.extend_from_slice(&dt.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    put_u64(&mut b, a.dg + 40, dl);
    b
}

/// A reader that errors once it has served `budget` reads.
struct Budget {
    inner: SliceRangeReader,
    left: u64,
}

impl ByteRangeReader for Budget {
    type Error = MdfError;
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        if self.left == 0 {
            return Err(MdfError::BlockSerializationError("read budget exhausted".into()));
        }
        self.left -= 1;
        self.inner.read_range(offset, length)
    }
}

fn build_via_reader(bytes: Vec<u8>) -> Result<MdfIndex, MdfError> {
    let size = bytes.len() as u64;
    let mut reader = Budget { inner: SliceRangeReader::new(bytes), left: 10_000 };
    MdfIndex::from_range_reader(&mut reader, size)
}

fn assert_cycle<T>(result: Result<T, MdfError>, what: &str) {
    match result {
        Err(e) => assert!(e.to_string().contains("cycle"), "{what}: expected a cycle error, got: {e}"),
        Ok(_) => panic!("{what}: a cyclic chain was accepted"),
    }
}

#[test]
fn range_reader_build_refuses_cyclic_data_group_chain() {
    let mut b = file_bytes();
    let a = addrs(&b);
    put_u64(&mut b, a.dg + 24, a.dg as u64); // dg_dg_next -> itself
    assert_cycle(build_via_reader(b), "##DG chain");
}

#[test]
fn range_reader_build_refuses_cyclic_channel_group_chain() {
    let mut b = file_bytes();
    let a = addrs(&b);
    put_u64(&mut b, a.cg + 24, a.cg as u64); // cg_cg_next -> itself
    assert_cycle(build_via_reader(b), "##CG chain");
}

#[test]
fn range_reader_build_refuses_cyclic_channel_chain() {
    let mut b = file_bytes();
    let a = addrs(&b);
    put_u64(&mut b, a.cn + 24, a.cn as u64); // cn_cn_next -> itself
    assert_cycle(build_via_reader(b), "##CN chain");
}

#[test]
fn range_reader_build_refuses_cyclic_data_list_chain() {
    assert_cycle(build_via_reader(with_cyclic_data_list(file_bytes())), "##DL chain (range reader)");
}

#[test]
fn local_builds_refuse_cyclic_data_list_chain() {
    let path = tmp("cyclic_dl.mf4");
    std::fs::write(&path, with_cyclic_data_list(file_bytes())).unwrap();
    assert_cycle(Mdf::from_file(&path).and_then(|m| m.values("Val", None)), "Mdf");
    assert_cycle(MdfIndex::from_file(&path), "MdfIndex::from_file");
}
