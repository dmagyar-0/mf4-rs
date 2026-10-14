//! Bus-logging regression tests: channel composition (structure members such
//! as `CAN_DataFrame.ID`), unsorted data groups (record-id multiplexed), VLSD
//! channel groups, and raw payload access.
//!
//! Fixtures:
//! * `pycan_canfd.mf4` — written by python-can's MF4Writer (asammdf backend):
//!   sorted, `##DZ`-compressed, composed `CAN_DataFrame` with a fixed 64-byte
//!   `DataBytes`. Frame i: id `(31 i) & 0x7FF`, bus `i % 2 + 1`, FD when i is
//!   even with length `[12,16,20,24,32,48,64][i % 7]`, else classic `i % 9`;
//!   payload byte k = `(i + k) & 0xFF`.
//! * `bus_unsorted.mf4` — logger-style UNSORTED file (record_id_len 1): CAN
//!   FD and Ethernet frames interleaved, payloads in VLSD channel groups.
//!   CAN frame n / ETH frame n payloads follow `can_payload` / `eth_payload`.

use mf4_rs::api::channel_group::ChannelGroup;
use mf4_rs::api::mdf::MDF;

fn fixture(f: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), f)
}

fn find<'a>(groups: &'a [ChannelGroup<'a>], name: &str) -> (&'a ChannelGroup<'a>, usize) {
    for g in groups {
        if let Some(i) = g.channels().iter().position(|c| c.name().unwrap().as_deref() == Some(name)) {
            return (g, i);
        }
    }
    panic!("channel {name} not found");
}

fn can_payload(i: usize) -> Vec<u8> {
    let fd = i % 3 == 0;
    let len = if fd { [8, 12, 16, 20, 24, 32, 48, 64][i % 8] } else { i % 9 };
    let v = (i * 7) & 0xFF;
    (0..len).map(|k| ((v + k * 13) & 0xFF) as u8).collect()
}

fn eth_payload(j: usize) -> Vec<u8> {
    let len = 46 + (j * 37) % 1455;
    (0..len).map(|k| ((j + k * 3) & 0xFF) as u8).collect()
}

#[test]
fn composition_children_are_listed_and_decoded() {
    let mdf = MDF::from_file(&fixture("pycan_canfd.mf4")).unwrap();
    let groups = mdf.channel_groups();
    let names: Vec<String> = groups[0].channels().iter().filter_map(|c| c.name().unwrap()).collect();
    for n in ["CAN_DataFrame", "CAN_DataFrame.ID", "CAN_DataFrame.DLC", "CAN_DataFrame.DataLength", "CAN_DataFrame.DataBytes", "CAN_DataFrame.EDL"] {
        assert!(names.iter().any(|x| x == n), "missing {n} in {names:?}");
    }
    let (g, id) = find(&groups, "CAN_DataFrame.ID");
    let (_, len) = find(&groups, "CAN_DataFrame.DataLength");
    let (_, edl) = find(&groups, "CAN_DataFrame.EDL");
    let cols = g.values_f64_many(&[id, len, edl]).unwrap();
    assert_eq!(cols[0].len(), 3000);
    for i in 0..3000 {
        let fd = i % 2 == 0;
        let l = if fd { [12, 16, 20, 24, 32, 48, 64][i % 7] } else { i % 9 };
        assert_eq!(cols[0][i], ((i * 31) & 0x7FF) as f64, "ID[{i}]");
        assert_eq!(cols[1][i], l as f64, "DataLength[{i}]");
        assert_eq!(cols[2][i], fd as u8 as f64, "EDL[{i}]");
    }
    // Fixed 64-byte payload column.
    let (_, db) = find(&groups, "CAN_DataFrame.DataBytes");
    let (data, offsets) = g.bytes_column(db).unwrap();
    assert_eq!(offsets.len(), 3001);
    assert_eq!(data.len(), 3000 * 64);
    for i in [0usize, 1, 2, 999, 2999] {
        let l = cols[1][i] as usize;
        let rec = &data[i * 64..i * 64 + l];
        let want: Vec<u8> = (0..l).map(|k| ((i + k) & 0xFF) as u8).collect();
        assert_eq!(rec, want.as_slice(), "payload[{i}]");
    }
}

#[test]
fn unsorted_logger_file_demultiplexes() {
    let mdf = MDF::from_file(&fixture("bus_unsorted.mf4")).unwrap();
    let groups = mdf.channel_groups();

    let (can, can_id) = find(&groups, "CAN_DataFrame.ID");
    let (_, can_len) = find(&groups, "CAN_DataFrame.DataLength");
    let (_, can_db) = find(&groups, "CAN_DataFrame.DataBytes");
    let cols = can.values_f64_many(&[can_id, can_len]).unwrap();
    assert_eq!(cols[0].len(), 2389);
    for i in 0..cols[0].len() {
        let want_id = if i % 17 == 0 { (i * 2654435761) & 0x1FFF_FFFF } else { (i * 31) & 0x7FF };
        assert_eq!(cols[0][i], want_id as f64, "CAN ID[{i}]");
        assert_eq!(cols[1][i], can_payload(i).len() as f64, "CAN DataLength[{i}]");
    }
    let (data, off) = can.bytes_column(can_db).unwrap();
    assert_eq!(off.len(), 2390);
    for i in 0..2389 {
        assert_eq!(&data[off[i] as usize..off[i + 1] as usize], can_payload(i).as_slice(), "CAN payload[{i}]");
    }

    let (eth, eth_db) = find(&groups, "ETH_Frame.DataBytes");
    let (_, eth_len) = find(&groups, "ETH_Frame.DataLength");
    let lens = &eth.values_f64_many(&[eth_len]).unwrap()[0];
    assert_eq!(lens.len(), 611);
    let (data, off) = eth.bytes_column(eth_db).unwrap();
    for j in 0..611 {
        let want = eth_payload(j);
        assert_eq!(lens[j], want.len() as f64, "ETH DataLength[{j}]");
        assert_eq!(&data[off[j] as usize..off[j + 1] as usize], want.as_slice(), "ETH payload[{j}]");
    }

    // VLSD values through the generic per-channel API agree with bytes_column.
    let chans = eth.channels();
    let vals = chans[eth_db].values().unwrap();
    assert_eq!(vals.len(), 611);

    // Master axes are monotonic per group.
    let t = &can.values_f64_many(&[can.master_index().unwrap()]).unwrap()[0];
    assert!(t.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn unsorted_demux_spills_to_disk_when_large() {
    // Force the spill path for every owned buffer in this process.
    unsafe { std::env::set_var("MF4RS_SPILL_BYTES", "1024") };
    let mdf = MDF::from_file(&fixture("bus_unsorted.mf4")).unwrap();
    let groups = mdf.channel_groups();
    let (eth, eth_db) = find(&groups, "ETH_Frame.DataBytes");
    let (data, off) = eth.bytes_column(eth_db).unwrap();
    assert_eq!(off.len(), 612);
    assert_eq!(&data[off[610] as usize..off[611] as usize], eth_payload(610).as_slice());
}

#[test]
fn unsorted_vlsd_zero_copy_offsets_mode() {
    // Never pack VLSD payloads: every payload is read in place via offsets.
    // (Both modes must give identical results; other tests may run with
    // either setting since tests share the process environment.)
    unsafe { std::env::set_var("MF4RS_VLSD_PACK_BYTES", "0") };
    let mdf = MDF::from_file(&fixture("bus_unsorted.mf4")).unwrap();
    let groups = mdf.channel_groups();
    let (can, can_db) = find(&groups, "CAN_DataFrame.DataBytes");
    let (data, off) = can.bytes_column(can_db).unwrap();
    assert_eq!(off.len(), 2390);
    for i in 0..2389 {
        assert_eq!(&data[off[i] as usize..off[i + 1] as usize], can_payload(i).as_slice(), "CAN payload[{i}]");
    }
    let (eth, eth_db) = find(&groups, "ETH_Frame.DataBytes");
    let (data, off) = eth.bytes_column(eth_db).unwrap();
    for j in 0..611 {
        assert_eq!(&data[off[j] as usize..off[j + 1] as usize], eth_payload(j).as_slice(), "ETH payload[{j}]");
    }
}

#[test]
fn ranged_payload_reads_match_full_reads() {
    for (file, name) in [
        ("bus_unsorted.mf4", "ETH_Frame.DataBytes"),
        ("bus_unsorted.mf4", "CAN_DataFrame.DataBytes"),
        ("bus_unsorted.mf4", "ETH_Frame.Source"),
        ("pycan_canfd.mf4", "CAN_DataFrame.DataBytes"),
    ] {
        let mdf = MDF::from_file(&fixture(file)).unwrap();
        let groups = mdf.channel_groups();
        let (g, i) = find(&groups, name);
        let (data, off) = g.bytes_column(i).unwrap();
        let n = off.len() - 1;
        for (a, z) in [(0, 1), (5, 17), (n - 3, n), (n - 1, n + 50), (n + 5, n + 9), (7, 7)] {
            let (d, o) = g.bytes_column_range(i, a, z).unwrap();
            let (a2, z2) = (a.min(n), z.min(n));
            assert_eq!(o.len(), z2 - a2 + 1, "{file}/{name} [{a},{z}) count");
            let want = &data[off[a2] as usize..off[z2] as usize];
            assert_eq!(d.as_slice(), want, "{file}/{name} [{a},{z}) bytes");
        }
    }
}
