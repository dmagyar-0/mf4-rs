//! Regression tests for `##HL` / `##DL` / `##DZ` chains and the single-pass
//! multi-channel decoder.
//!
//! Fixtures were written by asammdf 8.8 with `write_fragment_size=7000` so the
//! record data is split into many fragments:
//! * `frag_plain.mf4`     — `##DL` of plain `##DT` fragments + `##SD` chain
//! * `dz_deflate.mf4`     — `##HL` → `##DL` → `##DZ` (zip_type 0), VLSD `##SD` also compressed
//! * `dz_transposed.mf4`  — same with transposition + deflate (zip_type 1)
//!
//! Channels (n = 5000): `i16 = 3*i`, `f64 = 0.5*i`, `u8 = i % 255` with linear
//! conversion `2*x + 1`, `txt = "s{i % 97}"` (VLSD UTF-8), master `t = 0.01*i`.

use mf4_rs::api::mdf::MDF;
use mf4_rs::index::MdfIndex;
use mf4_rs::parsing::decoder::DecodedValue;

const N: usize = 5000;
const FIXTURES: [&str; 3] = ["frag_plain.mf4", "dz_deflate.mf4", "dz_transposed.mf4"];

fn path(f: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), f)
}

fn expected(name: &str, i: usize) -> f64 {
    match name {
        "i16" => (3 * i) as f64,
        "f64" => 0.5 * i as f64,
        "u8" => 2.0 * (i % 255) as f64 + 1.0,
        _ => unreachable!(),
    }
}

#[test]
fn direct_reader_decodes_all_layouts() {
    for f in FIXTURES {
        let mdf = MDF::from_file(&path(f)).unwrap();
        let groups = mdf.channel_groups();
        let g = groups
            .iter()
            .find(|g| g.channels().iter().any(|c| c.name().unwrap().as_deref() == Some("i16")))
            .expect("group with i16");
        let chans = g.channels();
        let idx = |n: &str| chans.iter().position(|c| c.name().unwrap().as_deref() == Some(n)).unwrap();

        for name in ["i16", "f64", "u8"] {
            let v = chans[idx(name)].values_as_f64().unwrap();
            assert_eq!(v.len(), N, "{f}/{name} length");
            for (i, x) in v.iter().enumerate() {
                assert_eq!(*x, expected(name, i), "{f}/{name}[{i}]");
            }
        }

        // One sweep for several channels gives identical results.
        let many = g.values_f64_many(&[idx("u8"), idx("i16"), idx("f64")]).unwrap();
        assert_eq!(many[0], chans[idx("u8")].values_as_f64().unwrap(), "{f} many/u8");
        assert_eq!(many[1], chans[idx("i16")].values_as_f64().unwrap(), "{f} many/i16");
        assert_eq!(many[2], chans[idx("f64")].values_as_f64().unwrap(), "{f} many/f64");

        // Master axis
        let t = chans[g.master_index().expect("master")].values_as_f64().unwrap();
        assert_eq!(t.len(), N);
        assert!((t[N - 1] - 0.01 * (N - 1) as f64).abs() < 1e-9, "{f} master");

        // VLSD strings (compressed ##SD chain in the DZ fixtures)
        let txt = chans[idx("txt")].values().unwrap();
        assert_eq!(txt.len(), N, "{f} txt length");
        for (i, v) in txt.iter().enumerate() {
            match v {
                Some(DecodedValue::String(s)) => assert_eq!(s, &format!("s{}", i % 97), "{f} txt[{i}]"),
                other => panic!("{f} txt[{i}]: {other:?}"),
            }
        }
    }
}

#[test]
fn index_reader_decodes_compressed_records() {
    for f in FIXTURES {
        let ix = MdfIndex::from_file(&path(f)).unwrap();
        let mut ix = ix.open_file(&path(f)).unwrap();
        for name in ["i16", "f64", "u8"] {
            let v = ix.values_f64(name).unwrap();
            assert_eq!(v.len(), N, "{f}/{name} index length");
            for (i, x) in v.iter().enumerate() {
                assert_eq!(*x, expected(name, i), "{f}/{name}[{i}] via index");
            }
        }
    }
}

#[test]
fn decompressed_buffer_is_cached() {
    let mdf = MDF::from_file(&path("dz_transposed.mf4")).unwrap();
    let groups = mdf.channel_groups();
    let g = groups.iter().find(|g| g.channels().len() > 2).unwrap();
    let dg = g.raw_data_group();
    let a = dg.data_blocks(g.mmap()).unwrap();
    let b = dg.data_blocks(g.mmap()).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].data.as_ptr(), b[0].data.as_ptr(), "second call must reuse the inflated buffer");
}
