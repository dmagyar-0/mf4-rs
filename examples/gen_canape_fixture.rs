//! Generate a large, CANape/DBC-flavoured MDF 4 fixture.
//!
//! Produces a ~100 MB file shaped like a CAN bus log that a measurement tool
//! (CANape and friends) would write from a DBC database:
//!
//! - **200 channel groups**, one per CAN message, each in its **own data
//!   group**, and each group's `##DT` block written *immediately* after its
//!   metadata cluster. That interleaving — metadata, data, metadata, data —
//!   is what real incremental writers produce, and it is the layout that makes
//!   a remote index walk expensive (each `next_dg_addr` hop lands past a
//!   multi-hundred-KB data block).
//! - **33 channels per group**: one master (`t`) plus 32 DBC-style signals,
//!   mostly small unsigned integers with a raw→physical factor/offset.
//! - **Every signal carries a `##CC` conversion** — linear for most, plus
//!   value-to-text enums (gear position, ignition state, system status), one
//!   rational and one algebraic per group.
//! - **Every signal carries a `##SI` source block** describing the bus
//!   (`source_type = 2 / BUS`, `bus_type = 2 / CAN`), with the bus name shared
//!   per group and a per-signal `Message.Signal` path, as a bus-logging tool
//!   would emit.
//! - Units are written as `##TX` blocks on every channel.
//!
//! Run it in release mode — it writes ~1.4 M records:
//!
//! ```text
//! cargo run --release --example gen_canape_fixture -- out.mf4 [groups] [records_per_group]
//! ```
//!
//! Defaults: `canape_like_100mb.mf4`, 200 groups, 7100 records per group.
//!
//! Two deliberate caveats, so the fixture is not mistaken for a full CANape
//! clone: signals are byte-aligned rather than bit-packed into an 8-byte CAN
//! payload (the writer lays channels out sequentially by byte), and no `##DZ`
//! compression or `##FH` history block is written — mf4-rs writes neither. At
//! the default size each group's data lands in a single `##DT` (well under the
//! 4 MB split threshold), so no `##DL` fragment chains appear; raise
//! `records_per_group` past ~61 700 if you want to exercise those too.

use mf4_rs::blocks::common::{BlockHeader, DataType};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::blocks::text_block::TextBlock;
use mf4_rs::error::MdfError;
use mf4_rs::writer::MdfWriter;

/// Link-section byte offsets inside a `##CN` block (24-byte header + 8 links).
const CN_LINK_SOURCE: u64 = 48;
const CN_LINK_UNIT: u64 = 72;

const DEFAULT_GROUPS: usize = 200;
const DEFAULT_RECORDS: usize = 7_100;

/// How a signal's raw value maps to a physical one.
enum Conv {
    /// `phys = offset + factor * raw`
    Linear { offset: f64, factor: f64 },
    /// Enumerated states, as a DBC value table becomes a `##CC` type 7.
    ValueToText(&'static [(i64, &'static str)], &'static str),
    /// `(p1*x² + p2*x + p3) / (p4*x² + p5*x + p6)`
    Rational([f64; 6]),
    /// MCD-2 MC text formula over `X`.
    Algebraic(&'static str),
}

struct SigSpec {
    name: &'static str,
    unit: &'static str,
    data_type: DataType,
    bytes: u32,
    conv: Conv,
}

const GEAR: &[(i64, &str)] = &[
    (0, "P"),
    (1, "R"),
    (2, "N"),
    (3, "D"),
    (4, "S"),
    (5, "M1"),
    (6, "M2"),
    (7, "M3"),
];
const IGNITION: &[(i64, &str)] = &[(0, "Off"), (1, "Acc"), (2, "Run"), (3, "Start")];
const STATUS: &[(i64, &str)] = &[(0, "OK"), (1, "Warning"), (2, "Fault")];

/// The 32-signal catalogue every message carries. Sizes are chosen so a
/// record is a tidy 68 bytes: 8 (master) + 12×1 + 12×2 + 4×2 + 4×4.
fn signal_catalogue() -> Vec<SigSpec> {
    use DataType::{SignedIntegerLE as I, UnsignedIntegerLE as U};
    macro_rules! lin {
        ($o:expr, $f:expr) => {
            Conv::Linear { offset: $o, factor: $f }
        };
    }
    vec![
        SigSpec { name: "EngSpeed",       unit: "rpm",   data_type: U, bytes: 2, conv: lin!(0.0, 0.25) },
        SigSpec { name: "VehSpeed",       unit: "km/h",  data_type: U, bytes: 2, conv: lin!(0.0, 0.01) },
        SigSpec { name: "CoolantTemp",    unit: "degC",  data_type: U, bytes: 1, conv: lin!(-40.0, 1.0) },
        SigSpec { name: "IntakeAirTemp",  unit: "degC",  data_type: U, bytes: 1, conv: lin!(-40.0, 1.0) },
        SigSpec { name: "ThrottlePos",    unit: "%",     data_type: U, bytes: 1, conv: lin!(0.0, 0.392157) },
        SigSpec { name: "AccelPedalPos",  unit: "%",     data_type: U, bytes: 1, conv: lin!(0.0, 0.4) },
        SigSpec { name: "EngLoad",        unit: "%",     data_type: U, bytes: 1, conv: lin!(0.0, 0.392157) },
        SigSpec { name: "FuelLevel",      unit: "%",     data_type: U, bytes: 1, conv: lin!(0.0, 0.4) },
        SigSpec { name: "MAP",            unit: "kPa",   data_type: U, bytes: 1, conv: lin!(0.0, 1.0) },
        SigSpec { name: "OilPressure",    unit: "bar",   data_type: U, bytes: 1, conv: lin!(0.0, 0.05) },
        SigSpec { name: "OilTemp",        unit: "degC",  data_type: U, bytes: 1, conv: lin!(-40.0, 1.0) },
        SigSpec { name: "FuelRate",       unit: "L/h",   data_type: U, bytes: 2, conv: lin!(0.0, 0.05) },
        SigSpec { name: "MAF",            unit: "g/s",   data_type: U, bytes: 2, conv: lin!(0.0, 0.01) },
        SigSpec { name: "BatteryVoltage", unit: "V",     data_type: U, bytes: 2, conv: lin!(0.0, 0.001) },
        SigSpec { name: "BoostPressure",  unit: "kPa",   data_type: U, bytes: 2, conv: lin!(-100.0, 0.1) },
        SigSpec { name: "WheelSpeedFL",   unit: "km/h",  data_type: U, bytes: 2, conv: lin!(0.0, 0.05625) },
        SigSpec { name: "WheelSpeedFR",   unit: "km/h",  data_type: U, bytes: 2, conv: lin!(0.0, 0.05625) },
        SigSpec { name: "WheelSpeedRL",   unit: "km/h",  data_type: U, bytes: 2, conv: lin!(0.0, 0.05625) },
        SigSpec { name: "WheelSpeedRR",   unit: "km/h",  data_type: U, bytes: 2, conv: lin!(0.0, 0.05625) },
        SigSpec { name: "EngTorque",      unit: "Nm",    data_type: I, bytes: 2, conv: lin!(0.0, 0.5) },
        SigSpec { name: "SteeringAngle",  unit: "deg",   data_type: I, bytes: 2, conv: lin!(0.0, 0.1) },
        SigSpec { name: "YawRate",        unit: "deg/s", data_type: I, bytes: 2, conv: lin!(0.0, 0.01) },
        SigSpec { name: "LatAccel",       unit: "m/s^2", data_type: I, bytes: 2, conv: lin!(0.0, 0.001) },
        SigSpec { name: "OdometerTotal",  unit: "km",    data_type: U, bytes: 4, conv: lin!(0.0, 0.125) },
        SigSpec { name: "EngRunTime",     unit: "s",     data_type: U, bytes: 4, conv: lin!(0.0, 1.0) },
        SigSpec { name: "TripFuelUsed",   unit: "L",     data_type: U, bytes: 4, conv: lin!(0.0, 0.001) },
        SigSpec { name: "DTCCount",       unit: "",      data_type: U, bytes: 4, conv: lin!(0.0, 1.0) },
        SigSpec { name: "GearPos",        unit: "",      data_type: U, bytes: 1, conv: Conv::ValueToText(GEAR, "Unknown") },
        SigSpec { name: "IgnitionState",  unit: "",      data_type: U, bytes: 1, conv: Conv::ValueToText(IGNITION, "Invalid") },
        SigSpec { name: "SystemStatus",   unit: "",      data_type: U, bytes: 1, conv: Conv::ValueToText(STATUS, "NotAvailable") },
        SigSpec { name: "LambdaSensor",   unit: "",      data_type: U, bytes: 2, conv: Conv::Rational([0.0, 1.0, 0.0, 0.0, 0.0, 1024.0]) },
        SigSpec { name: "PowerEstimate",  unit: "kW",    data_type: U, bytes: 2, conv: Conv::Algebraic("X*0.0736") },
    ]
}

/// CAN message names, DBC-style: `<ECU>_<Message>_<nnn>`.
fn message_name(group: usize) -> String {
    const ECUS: &[&str] = &["PT", "CH", "BD", "SAS", "ABS", "TCU", "BMS", "EMS", "VCU", "ADAS"];
    const MSGS: &[&str] = &[
        "EngineData",
        "VehicleDynamics",
        "PowertrainStatus",
        "BrakeStatus",
        "SteeringInfo",
        "BatteryState",
        "ThermalMgmt",
        "DriverInput",
        "GearboxStatus",
        "EnergyFlow",
    ];
    format!(
        "{}_{}_{:03}",
        ECUS[group % ECUS.len()],
        MSGS[(group / ECUS.len()) % MSGS.len()],
        group
    )
}

/// Serialise a `##SI` source block: 24-byte header + 3 links + 3 data bytes,
/// padded to 56 bytes.
fn source_block_bytes(
    name_addr: u64,
    path_addr: u64,
    comment_addr: u64,
) -> Result<Vec<u8>, MdfError> {
    let header = BlockHeader {
        id: "##SI".into(),
        reserved0: 0,
        block_len: 56,
        links_nr: 3,
    };
    let mut b = Vec::with_capacity(56);
    b.extend_from_slice(&header.to_bytes()?);
    b.extend_from_slice(&name_addr.to_le_bytes());
    b.extend_from_slice(&path_addr.to_le_bytes());
    b.extend_from_slice(&comment_addr.to_le_bytes());
    b.push(2); // source_type: 2 = BUS
    b.push(2); // bus_type:    2 = CAN
    b.push(0); // flags
    b.extend_from_slice(&[0u8; 5]); // reserved, pads to 56
    Ok(b)
}

/// Write a `##TX` block and return its file offset.
fn write_text(writer: &mut MdfWriter, id: &str, text: &str) -> Result<u64, MdfError> {
    let bytes = TextBlock::new(text).to_bytes()?;
    writer.write_block_with_id(&bytes, id)
}

/// Build a non-text `##CC` block for one signal.
fn conversion_block(
    cc_type: ConversionType,
    cc_val: Vec<f64>,
    cc_ref: Vec<u64>,
    name_addr: u64,
) -> ConversionBlock {
    ConversionBlock {
        header: BlockHeader {
            id: "##CC".into(),
            reserved0: 0,
            block_len: 0, // filled in by to_bytes()
            links_nr: 0,
        },
        cc_tx_name: Some(name_addr),
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref_count: cc_ref.len() as u16,
        cc_val_count: cc_val.len() as u16,
        cc_ref,
        cc_type,
        cc_precision: 0,
        cc_flags: 0,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val,
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
        default_conversion: None,
    }
}

/// Deterministic pseudo-random stream, so the fixture is byte-reproducible.
struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn main() -> Result<(), MdfError> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "canape_like_100mb.mf4".to_string());
    let groups: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_GROUPS);
    let records: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_RECORDS);

    let catalogue = signal_catalogue();
    // Record layout: master f64 first, then every signal byte-aligned in order.
    let mut offsets = Vec::with_capacity(catalogue.len());
    let mut cursor = 8u32;
    for spec in &catalogue {
        offsets.push(cursor);
        cursor += spec.bytes;
    }
    let record_size = cursor as usize;

    println!(
        "writing {path}: {groups} groups x {} channels x {records} records \
         ({record_size} B/record)",
        catalogue.len() + 1
    );
    let started = std::time::Instant::now();

    let mut writer = MdfWriter::new(&path)?;
    writer.init_mdf_file()?;

    let mut rng = Lcg(0x5EED_1234_ABCD_0001);
    let mut record = vec![0u8; record_size];

    for g in 0..groups {
        let message = message_name(g);
        let can_id = 0x100 + g;

        // --- channel group -------------------------------------------------
        // add_channel_group(None, ..) starts a fresh ##DG, so every message
        // gets its own data group, exactly as a per-message logger writes it.
        let cg = writer.add_channel_group(None, |_| {})?;
        writer.set_channel_group_name(&cg, &message)?;
        writer.set_channel_group_comment(
            &cg,
            &format!("CAN1 / id 0x{can_id:03X} / cycle 10 ms / from vehicle.dbc"),
        )?;

        // --- channels ------------------------------------------------------
        let master = writer.add_channel(&cg, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.bit_count = 64;
            ch.byte_offset = 0;
            ch.name = Some("t".to_string());
        })?;
        writer.set_time_channel(&master)?;

        let mut cn_ids = Vec::with_capacity(catalogue.len());
        let mut prev = master.clone();
        for (i, spec) in catalogue.iter().enumerate() {
            // Signal names are suffixed with the group index: a DBC keeps
            // signal names unique across messages, and unique names keep
            // name-based index lookups unambiguous.
            let signal_name = format!("{}_{:03}", spec.name, g);
            let byte_offset = offsets[i];
            let bits = spec.bytes * 8;
            let dt = spec.data_type.clone();
            let cn = writer.add_channel(&cg, Some(&prev), move |ch| {
                ch.data_type = dt;
                ch.bit_count = bits;
                ch.byte_offset = byte_offset;
                ch.name = Some(signal_name);
            })?;
            prev = cn.clone();
            cn_ids.push(cn);
        }

        // --- per-channel metadata: unit ##TX, ##CC conversion, ##SI source --
        // Written before this group's ##DT so each group forms one metadata
        // cluster followed by its data — the interleaved layout real tools
        // produce.
        let bus_tx = write_text(&mut writer, &format!("fx_bus_{g}"), "CAN1")?;

        let master_unit = write_text(&mut writer, &format!("fx_unit_{}", master), "s")?;
        let master_pos = writer.get_block_position(&master).ok_or_else(|| {
            MdfError::BlockSerializationError("master channel position missing".into())
        })?;
        writer.update_link(master_pos + CN_LINK_UNIT, master_unit)?;
        let master_path = write_text(
            &mut writer,
            &format!("fx_sipath_{}", master),
            &format!("{message}.t"),
        )?;
        let master_si = source_block_bytes(bus_tx, master_path, 0)?;
        let master_si_pos =
            writer.write_block_with_id(&master_si, &format!("fx_si_{}", master))?;
        writer.update_link(master_pos + CN_LINK_SOURCE, master_si_pos)?;

        for (i, spec) in catalogue.iter().enumerate() {
            let cn = &cn_ids[i];
            let cn_pos = writer.get_block_position(cn).ok_or_else(|| {
                MdfError::BlockSerializationError(format!("channel {cn} position missing"))
            })?;

            if !spec.unit.is_empty() {
                let unit = write_text(&mut writer, &format!("fx_unit_{cn}"), spec.unit)?;
                writer.update_link(cn_pos + CN_LINK_UNIT, unit)?;
            }

            // Conversion. The value-to-text case reuses the writer's helper,
            // which emits the text blocks and patches the ##CN link itself.
            match &spec.conv {
                Conv::ValueToText(mapping, default) => {
                    writer.add_value_to_text_conversion(mapping, default, Some(cn))?;
                }
                other => {
                    let cc_name = write_text(
                        &mut writer,
                        &format!("fx_ccname_{cn}"),
                        &format!("{}_conv", spec.name),
                    )?;
                    let (cc_type, cc_val, cc_ref) = match other {
                        Conv::Linear { offset, factor } => (
                            ConversionType::Linear,
                            vec![*offset, *factor],
                            Vec::new(),
                        ),
                        Conv::Rational(p) => (ConversionType::Rational, p.to_vec(), Vec::new()),
                        Conv::Algebraic(formula) => {
                            let tx = write_text(
                                &mut writer,
                                &format!("fx_ccformula_{cn}"),
                                formula,
                            )?;
                            (ConversionType::Algebraic, Vec::new(), vec![tx])
                        }
                        Conv::ValueToText(..) => unreachable!("handled above"),
                    };
                    let cc = conversion_block(cc_type, cc_val, cc_ref, cc_name);
                    let cc_bytes = cc.to_bytes()?;
                    let cc_pos =
                        writer.write_block_with_id(&cc_bytes, &format!("fx_cc_{cn}"))?;
                    // ##CN link slot 5 (offset 56) is cn_cc_conversion.
                    writer.update_link(cn_pos + 56, cc_pos)?;
                }
            }

            // Source: bus name shared per group, per-signal Message.Signal path.
            let si_path = write_text(
                &mut writer,
                &format!("fx_sipath_{cn}"),
                &format!("{message}.{}", spec.name),
            )?;
            let si = source_block_bytes(bus_tx, si_path, 0)?;
            let si_pos = writer.write_block_with_id(&si, &format!("fx_si_{cn}"))?;
            writer.update_link(cn_pos + CN_LINK_SOURCE, si_pos)?;
        }

        // --- data ----------------------------------------------------------
        writer.start_data_block_for_cg(&cg, 0)?;
        for r in 0..records {
            let t = r as f64 * 0.01; // 10 ms cycle
            record[0..8].copy_from_slice(&t.to_le_bytes());
            for (i, spec) in catalogue.iter().enumerate() {
                let at = offsets[i] as usize;
                let raw = rng.next_u32();
                match spec.bytes {
                    1 => {
                        // Keep enum signals inside their value table so the
                        // value-to-text conversions resolve to real states.
                        let v = match &spec.conv {
                            Conv::ValueToText(mapping, _) => (raw % (mapping.len() as u32 + 1)) as u8,
                            _ => raw as u8,
                        };
                        record[at] = v;
                    }
                    2 => {
                        let v = (raw & 0xFFFF) as u16;
                        record[at..at + 2].copy_from_slice(&v.to_le_bytes());
                    }
                    4 => {
                        record[at..at + 4].copy_from_slice(&raw.to_le_bytes());
                    }
                    n => unreachable!("unexpected signal width {n}"),
                }
            }
            writer.write_raw_record(&cg, &record)?;
        }
        writer.finish_data_block(&cg)?;

        if (g + 1) % 25 == 0 {
            println!("  {} / {groups} groups written", g + 1);
        }
    }

    writer.finalize()?;

    let size = std::fs::metadata(&path).map_err(MdfError::IOError)?.len();
    println!(
        "done: {path} = {size} bytes ({:.1} MiB), {} channels total, in {:.1?}",
        size as f64 / (1024.0 * 1024.0),
        groups * (catalogue.len() + 1),
        started.elapsed()
    );
    Ok(())
}
