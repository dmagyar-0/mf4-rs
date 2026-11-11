//! Guards the cross-language API contract (`docs/api-contract.json`).
//!
//! Two halves:
//!
//! * **Names** — the public methods of `Mdf`, `MdfIndex` and `MdfWriter`, the
//!   fields of the contract types and the crate-root re-exports are read from
//!   the source and compared with the contract. Adding, renaming or removing a
//!   public method fails this test until the contract (and with it the Python
//!   and TypeScript bindings, which have the same test) is updated.
//! * **Behaviour** — `Mdf`, `MdfIndex` and `MdfReader` must answer every
//!   contract call identically for the same file.

use std::collections::BTreeSet;
use std::path::Path;

use mf4_rs::{
    ChannelInfo, ConversionInfo, DataType, DecodedValue, GroupInfo, Mdf, MdfError, MdfIndex, MdfWriter,
    ReadOptions, Signal, TimeRange,
};
use serde_json::Value;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn contract() -> Value {
    let text = std::fs::read_to_string(root().join("docs/api-contract.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn strings(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

/// Names of the `pub fn`s in the inherent `impl` blocks of `type_name`.
fn public_methods(files: &[&str], type_name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        let mut inside = false;
        for line in text.lines() {
            if line.starts_with("impl") && !line.contains(" for ") {
                let rest = line.split_once(' ').map(|(_, r)| r).unwrap_or("");
                // `impl Name {`, `impl Name<'a> {`, `impl<'a, R: ..> Name<'a, R> {`
                let subject = rest.rsplit(' ').nth(1).unwrap_or("");
                inside = subject.split('<').next() == Some(type_name)
                    || rest.contains(&format!("> {type_name}<"))
                    || rest.starts_with(&format!("{type_name} "))
                    || rest.starts_with(&format!("{type_name}<"));
                continue;
            }
            if line == "}" {
                inside = false;
            }
            if inside {
                if let Some(rest) = line.strip_prefix("    pub fn ") {
                    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// Names of the `pub` fields of `pub struct type_name` in `file`.
fn public_fields(file: &str, type_name: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string(root().join(file)).unwrap();
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(&format!("pub struct {type_name} {{")) {
            inside = true;
            continue;
        }
        if line == "}" {
            inside = false;
        }
        if inside {
            if let Some(rest) = line.strip_prefix("    pub ") {
                out.insert(rest.split(':').next().unwrap().to_string());
            }
        }
    }
    out
}

/// What the contract says Rust exposes on a class: every contract name not
/// restricted to other languages, plus the Rust extras.
fn expected_class(spec: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for section in ["constructors", "accessors", "methods"] {
        for name in strings(&spec[section]) {
            let only = &spec["only"][&name];
            if only.is_null() || strings(only).contains("rust") {
                names.insert(name);
            }
        }
    }
    names.extend(strings(&spec["extra"]["rust"]));
    names
}

#[test]
fn class_methods_match_the_contract() {
    let contract = contract();
    let writer_files = [
        "src/writer/mdf_writer/init.rs",
        "src/writer/mdf_writer/data.rs",
        "src/writer/mdf_writer/io.rs",
        "src/writer/mdf_writer/vlsd.rs",
        "src/writer/mdf_writer/simple.rs",
    ];
    let cases: [(&str, &[&str]); 3] = [
        ("Mdf", &["src/api/mdf.rs"]),
        ("MdfIndex", &["src/index.rs"]),
        ("MdfWriter", &writer_files),
    ];
    for (class, files) in cases {
        let actual = public_methods(files, class);
        let expected = expected_class(&contract["classes"][class]);
        assert_eq!(
            actual, expected,
            "public methods of `{class}` differ from docs/api-contract.json \
             (left: source, right: contract). Update the contract and every binding together."
        );
    }
}

#[test]
fn reader_mirrors_the_index_read_api() {
    // `MdfReader` (an index bound to a custom byte source) must offer the
    // same data methods as `MdfIndex`.
    let reader = public_methods(&["src/index.rs"], "MdfReader");
    for name in ["read", "values", "time_range"] {
        assert!(reader.contains(name), "MdfReader is missing `{name}`");
    }
}

#[test]
fn type_fields_match_the_contract() {
    let contract = contract();
    let files = [
        ("Signal", "src/signal.rs"),
        ("GroupInfo", "src/types.rs"),
        ("ChannelInfo", "src/types.rs"),
        ("ConversionInfo", "src/types.rs"),
        ("ReadOptions", "src/types.rs"),
        ("TimeRange", "src/types.rs"),
    ];
    for (name, file) in files {
        let mut actual = public_fields(file, name);
        // `type` is a keyword in Rust; the field is serialized as `type`.
        if actual.remove("conversion_type") {
            actual.insert("type".to_string());
        }
        assert_eq!(
            actual,
            strings(&contract["types"][name]["fields"]),
            "fields of `{name}` differ from docs/api-contract.json"
        );
    }
}

#[test]
fn crate_root_exports_match_the_contract() {
    let lib = std::fs::read_to_string(root().join("src/lib.rs")).unwrap();
    let mut actual = BTreeSet::new();
    for line in lib.lines() {
        let Some(rest) = line.strip_prefix("pub use ") else { continue };
        let rest = rest.trim_end_matches(';');
        let items = match rest.split_once("::{") {
            Some((_, list)) => list.trim_end_matches('}').to_string(),
            None => rest.rsplit("::").next().unwrap().to_string(),
        };
        actual.extend(items.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    }
    assert_eq!(actual, strings(&contract()["module_exports"]["rust"]));
}

// ---------------------------------------------------------------------------
// Behaviour: the three Rust readers agree
// ---------------------------------------------------------------------------

fn write_fixture(path: &Path) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    w.set_start_time(1_700_000_000_000_000_000)?;
    for (group, base) in [("Engine", 0.0), ("Body", 100.0)] {
        let g = w.add_channel_group(Some(group))?;
        w.set_channel_group_comment(&g, "a comment")?;
        w.add_time_channel(&g, "Time")?;
        let speed = w.add_float_channel(&g, "Speed")?;
        let gear = w.add_int_channel(&g, "Gear")?;
        w.add_string_channel(&g, "Label")?;
        w.add_channel(&g, "Raw", DataType::SignedIntegerLE)?;
        w.add_linear_conversion(&speed, 1.0, 2.0)?;
        w.add_value_to_text_conversion(&gear, &[(0, "N"), (1, "D")], "?")?;
        w.start_data_block(&g)?;
        for i in 0..20u64 {
            w.write_record(
                &g,
                &[
                    DecodedValue::Float(i as f64 * 0.5),
                    DecodedValue::Float(base + i as f64),
                    DecodedValue::UnsignedInteger(i % 3),
                    DecodedValue::String(format!("L{i}")),
                    DecodedValue::SignedInteger(-(i as i64)),
                ],
            )?;
        }
        w.finish_data_block(&g)?;
    }
    w.finalize()
}

/// The contract's read surface, so the three readers can be driven alike.
trait Reader {
    fn groups(&mut self) -> Vec<GroupInfo>;
    fn group(&mut self, name: &str) -> Option<GroupInfo>;
    fn channel(&mut self, name: &str, group: Option<&str>) -> Option<ChannelInfo>;
    fn channel_names(&mut self) -> Vec<String>;
    fn start_time_ns(&mut self) -> Option<u64>;
    fn read(&mut self, name: &str, options: &ReadOptions) -> Result<Signal, MdfError>;
    fn values(&mut self, name: &str, options: &ReadOptions) -> Result<Vec<f64>, MdfError>;
    fn time_range(&mut self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError>;
    fn conversion(&mut self, name: &str, group: Option<&str>) -> Result<Option<ConversionInfo>, MdfError>;
}

macro_rules! impl_reader {
    ($ty:ty, $meta:ident) => {
        impl Reader for $ty {
            fn groups(&mut self) -> Vec<GroupInfo> {
                self.$meta().groups().unwrap()
            }
            fn group(&mut self, name: &str) -> Option<GroupInfo> {
                self.$meta().group(name).unwrap()
            }
            fn channel(&mut self, name: &str, group: Option<&str>) -> Option<ChannelInfo> {
                self.$meta().channel(name, group).unwrap()
            }
            fn channel_names(&mut self) -> Vec<String> {
                self.$meta().channel_names().unwrap()
            }
            fn start_time_ns(&mut self) -> Option<u64> {
                self.$meta().start_time_ns()
            }
            fn read(&mut self, name: &str, options: &ReadOptions) -> Result<Signal, MdfError> {
                self.read(name, options)
            }
            fn values(&mut self, name: &str, options: &ReadOptions) -> Result<Vec<f64>, MdfError> {
                self.values(name, options)
            }
            fn time_range(&mut self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError> {
                <$ty>::time_range(self, name, group)
            }
            fn conversion(&mut self, name: &str, group: Option<&str>) -> Result<Option<ConversionInfo>, MdfError> {
                self.$meta().conversion(name, group)
            }
        }
    };
}

trait Itself {
    fn itself(&self) -> &Self {
        self
    }
}
impl Itself for Mdf {}
impl Itself for MdfIndex {}

impl_reader!(Mdf, itself);
impl_reader!(MdfIndex, itself);
impl_reader!(mf4_rs::MdfReader<'_, mf4_rs::index::MmapRangeReader>, index);

fn same_floats(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn every_reader_gives_the_same_answers() -> Result<(), MdfError> {
    let dir = tempfile::tempdir().map_err(MdfError::IOError)?;
    let path = dir.path().join("contract.mf4");
    write_fixture(&path)?;
    let path = path.to_str().unwrap();

    let mut mdf = Mdf::from_file(path)?;
    let mut index = MdfIndex::from_file(path)?;
    // A JSON round trip must not change anything either.
    let mut reloaded = MdfIndex::from_json(&index.to_json()?)?;
    reloaded.validate()?;
    assert!(reloaded.source().is_none());
    reloaded.set_source(path);
    let bound_index = MdfIndex::from_bytes(std::fs::read(path).map_err(MdfError::IOError)?)?;
    let mut bound = bound_index.open_file(path)?;

    let option_sets = [
        ReadOptions::new(),
        ReadOptions::new().group("Body"),
        ReadOptions::new().time(Some(2.0), Some(4.5)),
        ReadOptions::new().group("Body").time(None, Some(1.0)),
        ReadOptions::new().time(Some(100.0), None),
        ReadOptions::new().records(3, 5),
        ReadOptions::new().group("Body").records(19, 1),
    ];

    let expected_groups = mdf.groups()?;
    assert_eq!(expected_groups.len(), 2);
    assert_eq!(expected_groups[0].master_channel.as_deref(), Some("Time"));
    assert_eq!(expected_groups[0].channel_names, ["Time", "Speed", "Gear", "Label", "Raw"]);
    assert_eq!(expected_groups[0].comment.as_deref(), Some("a comment"));
    let speed = mdf.channel("Speed", Some("Body"))?.expect("Speed");
    assert_eq!((speed.data_type.as_str(), speed.bit_count), ("FloatLE", 64));
    assert!(speed.has_conversion && !speed.is_master && !speed.is_vlsd);
    assert!(mdf.channel("Label", None)?.expect("Label").is_vlsd);

    let readers: [(&str, &mut dyn Reader); 3] =
        [("MdfIndex", &mut index), ("MdfIndex (from JSON)", &mut reloaded), ("MdfReader", &mut bound)];
    for (label, reader) in readers {
        assert_eq!(reader.groups(), expected_groups, "{label}: groups");
        assert_eq!(reader.group("Body"), Reader::group(&mut mdf, "Body"), "{label}: group");
        assert_eq!(reader.group("Nope"), None, "{label}: missing group");
        assert_eq!(reader.channel_names(), Reader::channel_names(&mut mdf), "{label}: channel_names");
        assert_eq!(reader.start_time_ns(), Some(1_700_000_000_000_000_000), "{label}: start time");
        assert_eq!(reader.channel("Nope", None), None, "{label}: missing channel");

        for name in ["Time", "Speed", "Gear", "Label", "Raw"] {
            for group in [None, Some("Body")] {
                assert_eq!(
                    reader.channel(name, group),
                    Reader::channel(&mut mdf, name, group),
                    "{label}: channel {name}"
                );
                assert_eq!(
                    reader.time_range(name, group)?,
                    Reader::time_range(&mut mdf, name, group)?,
                    "{label}: time_range {name}"
                );
                assert_eq!(
                    reader.conversion(name, group)?,
                    Reader::conversion(&mut mdf, name, group)?,
                    "{label}: conversion {name}"
                );
            }
            for options in &option_sets {
                let want = Reader::read(&mut mdf, name, options)?;
                let got = reader.read(name, options)?;
                assert_eq!((&got.name, &got.unit), (&want.name, &want.unit), "{label}: {name} {options:?}");
                assert_eq!(got.values, want.values, "{label}: values of {name} {options:?}");
                assert!(same_floats(&got.timestamps, &want.timestamps), "{label}: timestamps of {name} {options:?}");
                assert!(
                    same_floats(&reader.values(name, options)?, &Reader::values(&mut mdf, name, options)?),
                    "{label}: values() of {name} {options:?}"
                );
            }
        }

        // The one error rule: data operations on a missing channel fail with
        // ChannelNotFound; invalid windows with InvalidArgument.
        let missing = ReadOptions::new().group("Engine");
        assert!(matches!(reader.read("Nope", &missing), Err(MdfError::ChannelNotFound { .. })), "{label}");
        assert!(matches!(reader.values("Nope", &missing), Err(MdfError::ChannelNotFound { .. })), "{label}");
        assert!(matches!(reader.time_range("Nope", None), Err(MdfError::ChannelNotFound { .. })), "{label}");
        assert!(matches!(reader.conversion("Nope", None), Err(MdfError::ChannelNotFound { .. })), "{label}");
        let both = ReadOptions { start_time: Some(0.0), start_record: Some(0), ..ReadOptions::default() };
        assert!(matches!(reader.read("Speed", &both), Err(MdfError::InvalidArgument(_))), "{label}");
        let past_end = ReadOptions::new().records(15, 10);
        assert!(matches!(reader.read("Speed", &past_end), Err(MdfError::InvalidArgument(_))), "{label}");
    }

    // Spot-check the actual numbers once, against the reference reader.
    let window = mdf.read("Speed", &ReadOptions::new().time(Some(2.0), Some(3.0)))?;
    assert_eq!(window.timestamps, [2.0, 2.5, 3.0]);
    assert_eq!(window.values_f64(), [9.0, 11.0, 13.0]); // 1 + 2 * raw
    assert_eq!(mdf.read("Time", None)?.timestamps, Vec::<f64>::new(), "a master has no separate time axis");
    assert_eq!(mdf.time_range("Speed", None)?, Some(TimeRange { start: 0.0, end: 9.5 }));
    let gear = mdf.conversion("Gear", None)?.expect("Gear conversion");
    assert_eq!(gear.conversion_type, "ValueToText");
    assert_eq!(gear.texts.get(&2).map(String::as_str), Some("?"));
    assert_eq!(mdf.conversion("Time", None)?, None);
    assert_eq!(index.byte_ranges("Speed", None)?.len(), 1);
    assert_eq!(index.byte_ranges_for_records("Speed", 2, 3, Some("Body"))?.len(), 1);
    Ok(())
}
