//! Python bindings for mf4-rs using PyO3
//!
//! This module provides Python bindings for the main functionality of mf4-rs:
//! - Parsing MDF files
//! - Writing MDF files
//! - Creating and using indexes

use pyo3::prelude::*;
use pyo3::types::IntoPyDict;
use pyo3::{create_exception, wrap_pyfunction};
use numpy::{PyArray1, PyReadonlyArray1};
use pyo3_stub_gen::derive::{
    gen_stub_pyclass, gen_stub_pyclass_enum, gen_stub_pyfunction, gen_stub_pymethods,
};
use pyo3_stub_gen::define_stub_info_gatherer;
use std::collections::HashMap;

use crate::api::mdf::MDF;
use crate::writer::{MdfWriter, ColumnData};
use crate::index::{IndexedChannel, MdfIndex};
use crate::blocks::common::DataType;
use crate::parsing::decoder::DecodedValue;
use crate::error::MdfError;
use crate::block_layout::{BlockInfo, FileLayout, GapInfo, LinkInfo};

// Custom exception for MDF errors
create_exception!(mf4_rs, MdfException, pyo3::exceptions::PyException);

// Convert Rust MdfError to Python exception
impl From<MdfError> for PyErr {
    fn from(err: MdfError) -> PyErr {
        MdfException::new_err(format!("{:?}", err))
    }
}

/// MDF channel data type descriptor.
///
/// Wraps the on-disk MDF 4 data type code (`cn_data_type`) together with a
/// human-readable name. Use the ``create_data_type_*`` helper functions to
/// construct one for the writer API.
///
/// Attributes
/// ----------
/// name : str
///     Symbolic name, e.g. ``"FloatLE"``, ``"UnsignedIntegerLE"``.
/// value : int
///     The MDF spec numeric code (0-16, or 255 for unknown).
#[gen_stub_pyclass]
#[pyclass(name = "DataType")]
#[derive(Debug, Clone)]
pub struct PyDataType {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub value: u8,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyDataType {
    fn __str__(&self) -> String {
        self.name.clone()
    }
    
    fn __repr__(&self) -> String {
        format!("DataType({})", self.name)
    }
}

impl From<&DataType> for PyDataType {
    fn from(dt: &DataType) -> Self {
        let (name, value) = match dt {
            DataType::UnsignedIntegerLE => ("UnsignedIntegerLE", 0),
            DataType::UnsignedIntegerBE => ("UnsignedIntegerBE", 1),
            DataType::SignedIntegerLE => ("SignedIntegerLE", 2),
            DataType::SignedIntegerBE => ("SignedIntegerBE", 3),
            DataType::FloatLE => ("FloatLE", 4),
            DataType::FloatBE => ("FloatBE", 5),
            DataType::StringLatin1 => ("StringLatin1", 6),
            DataType::StringUtf8 => ("StringUtf8", 7),
            DataType::StringUtf16LE => ("StringUtf16LE", 8),
            DataType::StringUtf16BE => ("StringUtf16BE", 9),
            DataType::ByteArray => ("ByteArray", 10),
            DataType::MimeSample => ("MimeSample", 11),
            DataType::MimeStream => ("MimeStream", 12),
            DataType::CanOpenDate => ("CanOpenDate", 13),
            DataType::CanOpenTime => ("CanOpenTime", 14),
            DataType::ComplexLE => ("ComplexLE", 15),
            DataType::ComplexBE => ("ComplexBE", 16),
            DataType::Unknown(_) => ("Unknown", 255),
        };
        PyDataType {
            name: name.to_string(),
            value,
        }
    }
}

impl From<PyDataType> for DataType {
    fn from(py_dt: PyDataType) -> Self {
        match py_dt.value {
            0 => DataType::UnsignedIntegerLE,
            1 => DataType::UnsignedIntegerBE,
            2 => DataType::SignedIntegerLE,
            3 => DataType::SignedIntegerBE,
            4 => DataType::FloatLE,
            5 => DataType::FloatBE,
            6 => DataType::StringLatin1,
            7 => DataType::StringUtf8,
            8 => DataType::StringUtf16LE,
            9 => DataType::StringUtf16BE,
            10 => DataType::ByteArray,
            11 => DataType::MimeSample,
            12 => DataType::MimeStream,
            13 => DataType::CanOpenDate,
            14 => DataType::CanOpenTime,
            15 => DataType::ComplexLE,
            16 => DataType::ComplexBE,
            _ => DataType::Unknown(()),
        }
    }
}

/// A single decoded channel sample, tagged with its underlying type.
///
/// Returned by ``create_*_value`` helpers and consumed by
/// :py:meth:`MdfWriter.write_record`. When *reading*, most APIs return native
/// Python objects (``float`` / ``int`` / ``str`` / ``bytes``) directly, so you
/// usually only construct ``DecodedValue`` to feed back into the writer.
///
/// Variants carry a single ``value`` field (use the ``value`` getter to
/// retrieve the inner native Python value):
///
/// - ``Float(value: float)``
/// - ``UnsignedInteger(value: int)``
/// - ``SignedInteger(value: int)``
/// - ``String(value: str)``
/// - ``ByteArray(value: bytes)``
/// - ``Unknown()`` — undecodable / not yet supported
#[gen_stub_pyclass_enum]
#[pyclass(name = "DecodedValue")]
#[derive(Debug, Clone)]
pub enum PyDecodedValue {
    Float { value: f64 },
    UnsignedInteger { value: u64 },
    SignedInteger { value: i64 },
    String { value: String },
    ByteArray { value: Vec<u8> },
    Unknown { },
}

// NOTE: `gen_stub_pymethods` is intentionally not applied here. Combining
// it with `gen_stub_pyclass_enum` on a complex enum trips an internal
// assertion in pyo3-stub-gen 0.7. The runtime methods (`__str__`,
// `__repr__`, `value` getter) still work — they're just not described in
// the generated stub. Enum variants and the class itself are.
#[pymethods]
impl PyDecodedValue {
    fn __str__(&self) -> String {
        match self {
            PyDecodedValue::Float { value } => format!("{}", value),
            PyDecodedValue::UnsignedInteger { value } => format!("{}", value),
            PyDecodedValue::SignedInteger { value } => format!("{}", value),
            PyDecodedValue::String { value } => value.clone(),
            PyDecodedValue::ByteArray { value } => format!("bytes[{}]", value.len()),
            PyDecodedValue::Unknown { } => "Unknown".to_string(),
        }
    }
    
    fn __repr__(&self) -> String {
        match self {
            PyDecodedValue::Float { value } => format!("Float({})", value),
            PyDecodedValue::UnsignedInteger { value } => format!("UnsignedInteger({})", value),
            PyDecodedValue::SignedInteger { value } => format!("SignedInteger({})", value),
            PyDecodedValue::String { value } => format!("String('{}')", value),
            PyDecodedValue::ByteArray { value } => format!("ByteArray({})", value.len()),
            PyDecodedValue::Unknown { } => "Unknown".to_string(),
        }
    }
    
    #[getter]
    fn value(&self, py: Python) -> PyObject {
        match self {
            PyDecodedValue::Float { value } => value.to_object(py),
            PyDecodedValue::UnsignedInteger { value } => value.to_object(py),
            PyDecodedValue::SignedInteger { value } => value.to_object(py),
            PyDecodedValue::String { value } => value.to_object(py),
            PyDecodedValue::ByteArray { value } => value.to_object(py),
            PyDecodedValue::Unknown { } => py.None(),
        }
    }
}

impl From<DecodedValue> for PyDecodedValue {
    fn from(dv: DecodedValue) -> Self {
        match dv {
            DecodedValue::Float(v) => PyDecodedValue::Float { value: v },
            DecodedValue::UnsignedInteger(v) => PyDecodedValue::UnsignedInteger { value: v },
            DecodedValue::SignedInteger(v) => PyDecodedValue::SignedInteger { value: v },
            DecodedValue::String(v) => PyDecodedValue::String { value: v },
            DecodedValue::ByteArray(v) => PyDecodedValue::ByteArray { value: v },
            DecodedValue::MimeSample(v) => PyDecodedValue::ByteArray { value: v },
            DecodedValue::MimeStream(v) => PyDecodedValue::ByteArray { value: v },
            DecodedValue::Unknown => PyDecodedValue::Unknown { },
        }
    }
}

impl From<PyDecodedValue> for DecodedValue {
    fn from(py_dv: PyDecodedValue) -> Self {
        match py_dv {
            PyDecodedValue::Float { value } => DecodedValue::Float(value),
            PyDecodedValue::UnsignedInteger { value } => DecodedValue::UnsignedInteger(value),
            PyDecodedValue::SignedInteger { value } => DecodedValue::SignedInteger(value),
            PyDecodedValue::String { value } => DecodedValue::String(value),
            PyDecodedValue::ByteArray { value } => DecodedValue::ByteArray(value),
            PyDecodedValue::Unknown { } => DecodedValue::Unknown,
        }
    }
}

/// Direct conversion from DecodedValue to PyObject without intermediate allocation.
/// This is more efficient than creating a DecodedValue and immediately extracting its value.
fn decoded_value_to_pyobject(dv: DecodedValue, py: Python) -> PyObject {
    match dv {
        DecodedValue::Float(v) => v.to_object(py),
        DecodedValue::UnsignedInteger(v) => v.to_object(py),
        DecodedValue::SignedInteger(v) => v.to_object(py),
        DecodedValue::String(v) => v.to_object(py),
        DecodedValue::ByteArray(v) | DecodedValue::MimeSample(v) | DecodedValue::MimeStream(v) => {
            v.to_object(py)
        }
        DecodedValue::Unknown => py.None(),
    }
}

/// Read-only metadata describing a single channel.
///
/// Found on :py:attr:`GroupInfo.channels`, and returned by
/// :py:meth:`Mdf.channel` / :py:meth:`MdfIndex.channel`.
///
/// Attributes
/// ----------
/// name : Optional[str]
///     Channel name from the ``##TX`` block, or ``None`` if not set.
/// unit : Optional[str]
///     Engineering unit string, or ``None``.
/// comment : Optional[str]
///     Free-form comment, or ``None``. Always ``None`` when obtained via the
///     index (not stored in `IndexedChannel`).
/// data_type : DataType
///     The MDF data type of the raw samples.
/// bit_count : int
///     Width of the raw value in bits (e.g. 32 for f32, 64 for f64/u64).
#[gen_stub_pyclass]
#[pyclass(name = "ChannelInfo")]
#[derive(Debug, Clone)]
pub struct PyChannelInfo {
    #[pyo3(get)]
    pub name: Option<String>,
    #[pyo3(get)]
    pub unit: Option<String>,
    #[pyo3(get)]
    pub comment: Option<String>,
    #[pyo3(get)]
    pub data_type: PyDataType,
    #[pyo3(get)]
    pub bit_count: u32,
    /// True if this is the group's master / time channel.
    #[pyo3(get)]
    pub is_master: bool,
    /// True if this is a variable-length (VLSD) channel.
    #[pyo3(get)]
    pub is_vlsd: bool,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyChannelInfo {
    fn __str__(&self) -> String {
        format!("Channel(name={:?}, data_type={}, bit_count={})",
                self.name, self.data_type.name, self.bit_count)
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl PyChannelInfo {
    /// Build from a parsed (live-file) channel.
    fn from_channel(channel: &crate::api::channel::Channel<'_>) -> PyResult<Self> {
        let block = channel.block();
        Ok(PyChannelInfo {
            name: channel.name()?,
            unit: channel.unit()?,
            comment: channel.comment()?,
            data_type: PyDataType::from(&block.data_type),
            bit_count: block.bit_count,
            is_master: block.channel_type == 2,
            is_vlsd: block.channel_type == 1 && block.data != 0,
        })
    }

    /// Build from an indexed channel (comments are not stored in the index).
    fn from_indexed(channel: &IndexedChannel) -> Self {
        PyChannelInfo {
            name: channel.name.clone(),
            unit: channel.unit.clone(),
            comment: None,
            data_type: PyDataType::from(&channel.data_type),
            bit_count: channel.bit_count,
            is_master: channel.is_master(),
            is_vlsd: channel.is_vlsd(),
        }
    }
}

/// Read-only metadata describing a channel group (``##CG`` block).
///
/// Returned by :py:attr:`Mdf.groups` / :py:attr:`MdfIndex.groups`.
///
/// Attributes
/// ----------
/// name : Optional[str]
///     Group / acquisition name (often ``None`` for files written by mf4-rs).
/// comment : Optional[str]
///     Free-form comment.
/// channel_count : int
///     Number of channels in this group.
/// record_count : int
///     Number of records (cycles) recorded for this group.
#[gen_stub_pyclass]
#[pyclass(name = "GroupInfo")]
#[derive(Debug, Clone)]
pub struct PyChannelGroupInfo {
    #[pyo3(get)]
    pub name: Option<String>,
    #[pyo3(get)]
    pub comment: Option<String>,
    #[pyo3(get)]
    pub channel_count: usize,
    #[pyo3(get)]
    pub record_count: u64,
    /// Metadata for every channel in this group, in record order.
    #[pyo3(get)]
    pub channels: Vec<PyChannelInfo>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyChannelGroupInfo {
    /// Find a channel in this group by name (first match), or ``None``.
    fn channel(&self, name: &str) -> Option<PyChannelInfo> {
        self.channels
            .iter()
            .find(|c| c.name.as_deref() == Some(name))
            .cloned()
    }

    /// Names of every named channel in this group.
    #[getter]
    fn channel_names(&self) -> Vec<String> {
        self.channels.iter().filter_map(|c| c.name.clone()).collect()
    }

    fn __str__(&self) -> String {
        format!("Group(name={:?}, channels={}, records={})",
                self.name, self.channel_count, self.record_count)
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl PyChannelGroupInfo {
    /// Build from a live (parsed) channel group.
    fn from_group(group: &crate::api::channel_group::ChannelGroup<'_>) -> PyResult<Self> {
        let channels = group
            .channels()
            .iter()
            .map(PyChannelInfo::from_channel)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PyChannelGroupInfo {
            name: group.name()?,
            comment: group.comment()?,
            channel_count: channels.len(),
            record_count: group.raw_channel_group().block.cycles_nr,
            channels,
        })
    }

    /// Build from an indexed channel group.
    fn from_indexed(group: &crate::index::IndexedChannelGroup) -> Self {
        let channels: Vec<PyChannelInfo> =
            group.channels.iter().map(PyChannelInfo::from_indexed).collect();
        PyChannelGroupInfo {
            name: group.name.clone(),
            comment: group.comment.clone(),
            channel_count: channels.len(),
            record_count: group.record_count,
            channels,
        }
    }
}

/// Helper function to check if pandas is available and import it
fn check_pandas_available(py: Python) -> PyResult<PyObject> {
    py.import_bound("pandas")
        .map(|m| m.into())
        .map_err(|_| MdfException::new_err(
            "Pandas is not installed. Please install pandas to use Series-based methods."
        ))
}

/// Parse a ``__getitem__`` key into ``(channel_name, optional_group_name)``.
///
/// Accepts a plain ``str`` (``obj["Speed"]``) or a ``(name, group)`` tuple
/// (``obj["Speed", "Engine"]``) for disambiguating duplicate channel names.
fn parse_lookup_key(key: &Bound<'_, PyAny>) -> PyResult<(String, Option<String>)> {
    if let Ok((name, group)) = key.extract::<(String, String)>() {
        Ok((name, Some(group)))
    } else if let Ok(name) = key.extract::<String>() {
        Ok((name, None))
    } else {
        Err(MdfException::new_err(
            "channel key must be a name string or a (name, group) tuple",
        ))
    }
}

/// Build a ``pandas.Series`` from a decoded [`Signal`].
///
/// `values` becomes the data; `timestamps` (master values in seconds) becomes
/// the index. With a `start_time_ns` the index is a ``DatetimeIndex`` (relative
/// seconds added to the file start); otherwise the raw seconds are used. When
/// `timestamps` is empty (no master, or the channel *is* the master) a default
/// integer index is used. The series ``name`` is set to the channel name.
fn signal_to_series(
    py: Python,
    pd: &PyObject,
    name: &str,
    timestamps: &[f64],
    values: Vec<Option<DecodedValue>>,
    start_time_ns: Option<u64>,
) -> PyResult<PyObject> {
    let py_values: Vec<PyObject> = values
        .into_iter()
        .map(|o| o.map(|dv| decoded_value_to_pyobject(dv, py)).unwrap_or_else(|| py.None()))
        .collect();

    let index: PyObject = if timestamps.is_empty() {
        py.None()
    } else {
        let py_ts = PyArray1::from_vec_bound(py, timestamps.to_vec());
        if let Some(start_ns) = start_time_ns {
            // Vectorized: start_timestamp + to_timedelta(seconds).
            let to_datetime = pd.getattr(py, "to_datetime")?;
            let to_timedelta = pd.getattr(py, "to_timedelta")?;
            let start_ts = to_datetime.call(
                py, (start_ns,),
                Some([("unit", "ns")].into_py_dict(py)),
            )?;
            let deltas = to_timedelta.call(
                py, (py_ts.clone(),),
                Some([("unit", "s")].into_py_dict(py)),
            )?;
            match deltas.call_method1(py, "__add__", (start_ts,)) {
                Ok(dt_index) => dt_index,
                Err(_) => py_ts.into(),
            }
        } else {
            py_ts.into()
        }
    };

    let series_class = pd.getattr(py, "Series")?;
    let series = if index.is_none(py) {
        series_class.call1(py, (py_values,))?
    } else {
        series_class.call(py, (py_values,), Some([("index", index)].into_py_dict(py)))?
    };
    series.setattr(py, "name", name)?;
    Ok(series)
}

/// Read-only handle to an MDF 4 file, backed by a memory-mapped buffer.
///
/// Metadata (block tree, channel names, conversions) is parsed up front, but
/// sample data is only decoded when you call :py:meth:`read`, :py:meth:`read_raw`
/// or :py:meth:`series`. Navigate by group / channel **name** — there are no
/// numeric indices in the public API.
///
/// Example
/// -------
/// >>> import mf4_rs
/// >>> mdf = mf4_rs.Mdf("recording.mf4")
/// >>> for group in mdf.groups:
/// ...     print(group.name, group.record_count, group.channel_names)
/// >>> speed = mdf["Speed"]                 # numpy float64 array
/// >>> rpm   = mdf.read("RPM", group="Engine")
/// >>> s     = mdf.series("Speed")          # pandas Series, datetime index
#[gen_stub_pyclass]
#[pyclass(name = "Mdf")]
pub struct PyMDF {
    mdf: Box<MDF>,
    path: String,
}

impl PyMDF {
    /// Locate a live channel group + channel index by (optional group) name.
    fn find_group_channel<'a>(
        &'a self,
        group: Option<&str>,
        name: &str,
    ) -> PyResult<(crate::api::channel_group::ChannelGroup<'a>, usize)> {
        for g in self.mdf.channel_groups() {
            if let Some(gn) = group {
                if g.name()?.as_deref() != Some(gn) {
                    continue;
                }
            }
            let chans = g.channels();
            for (i, ch) in chans.iter().enumerate() {
                if ch.name()?.as_deref() == Some(name) {
                    return Ok((g, i));
                }
            }
        }
        Err(MdfException::new_err(match group {
            Some(gn) => format!("Channel '{}' not found in group '{}'", name, gn),
            None => format!("Channel '{}' not found", name),
        }))
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMDF {
    /// Open and parse an MDF 4 file from disk.
    ///
    /// Parameters
    /// ----------
    /// path : str
    ///     Path to a ``.mf4`` / ``.mdf`` file. Must be MDF version >= 4.10.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If the file does not exist, has the wrong magic bytes, an
    ///     unsupported version, or contains malformed blocks.
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let mdf = Box::new(MDF::from_file(path)?);
        Ok(PyMDF { mdf, path: path.to_string() })
    }

    /// The file path this reader was opened from.
    #[getter]
    fn source(&self) -> String {
        self.path.clone()
    }

    /// A flat catalog of every channel as ``(source, group, channel)`` tuples.
    ///
    /// ``source`` is this file's path (same for every row). ``group`` /
    /// ``channel`` are ``None`` if unnamed. Metadata only — no samples decoded.
    ///
    /// Returns
    /// -------
    /// list[tuple[str, Optional[str], Optional[str]]]
    fn list_signals(&self) -> PyResult<Vec<(String, Option<String>, Option<String>)>> {
        let mut out = Vec::new();
        for group in self.mdf.channel_groups() {
            let gname = group.name()?;
            for channel in group.channels() {
                out.push((self.path.clone(), gname.clone(), channel.name()?));
            }
        }
        Ok(out)
    }

    /// Metadata for every channel group in the file, in file order.
    ///
    /// Each :class:`GroupInfo` carries its ``channels`` (a list of
    /// :class:`ChannelInfo`), so a single ``mdf.groups`` call gives the whole
    /// structure.
    #[getter]
    fn groups(&self) -> PyResult<Vec<PyChannelGroupInfo>> {
        self.mdf
            .channel_groups()
            .iter()
            .map(PyChannelGroupInfo::from_group)
            .collect()
    }

    /// Find a channel group by name (first match), or ``None``.
    fn group(&self, name: &str) -> PyResult<Option<PyChannelGroupInfo>> {
        for g in self.mdf.channel_groups() {
            if g.name()?.as_deref() == Some(name) {
                return Ok(Some(PyChannelGroupInfo::from_group(&g)?));
            }
        }
        Ok(None)
    }

    /// Find a channel by name across all groups (first match), or ``None``.
    fn channel(&self, name: &str) -> PyResult<Option<PyChannelInfo>> {
        for g in self.mdf.channel_groups() {
            for ch in g.channels() {
                if ch.name()?.as_deref() == Some(name) {
                    return Ok(Some(PyChannelInfo::from_channel(&ch)?));
                }
            }
        }
        Ok(None)
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[getter]
    fn channel_names(&self) -> PyResult<Vec<String>> {
        let mut names = Vec::new();
        for g in self.mdf.channel_groups() {
            for ch in g.channels() {
                if let Some(n) = ch.name()? {
                    names.push(n);
                }
            }
        }
        Ok(names)
    }

    /// Read a channel as a ``pandas.Series`` of values indexed by timestamps.
    ///
    /// This is the primary read: the channel's samples (with all conversions
    /// applied) are the data, and the group's master/time channel is the index
    /// — a ``DatetimeIndex`` when the file has a start time, otherwise the raw
    /// master seconds. If there is no master, or the requested channel *is* the
    /// master, a default integer index is used.
    ///
    /// Parameters
    /// ----------
    /// name : str
    ///     Channel name (case-sensitive).
    /// group : Optional[str]
    ///     Restrict the search to a single group by name. Use this when the
    ///     same channel name (e.g. ``"Time"``) appears in several groups.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If no matching channel exists or pandas is not installed.
    fn read(&self, py: Python, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        let pd = check_pandas_available(py)?;
        let start_time_ns = self.mdf.start_time_ns();
        let signal = match group {
            Some(gn) => self
                .mdf
                .group(gn)
                .ok_or_else(|| MdfException::new_err(format!("Channel group '{}' not found", gn)))?
                .signal(name)?,
            None => self.mdf.signal(name)?,
        };
        let signal = signal.ok_or_else(|| {
            MdfException::new_err(match group {
                Some(gn) => format!("Channel '{}' not found in group '{}'", name, gn),
                None => format!("Channel '{}' not found", name),
            })
        })?;
        signal_to_series(py, &pd, &signal.name, &signal.timestamps, signal.values, start_time_ns)
    }

    /// Read a numeric channel by name as a plain numpy ``float64`` array.
    ///
    /// The fast, pandas-free path — just the values, no timestamp index.
    /// Invalid / non-numeric samples are ``NaN``. Use :py:meth:`read` when you
    /// want the timestamp-indexed Series and faithful (text / table) conversions.
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    fn values<'py>(&self, py: Python<'py>, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        let (g, idx) = self.find_group_channel(group, name)?;
        let values = g.channels()[idx].values_as_f64()?;
        Ok(PyArray1::from_vec_bound(py, values).into())
    }

    /// ``mdf["Speed"]`` — shorthand for :py:meth:`read` (timestamp-indexed Series).
    ///
    /// Pass a ``(name, group)`` tuple to disambiguate a channel name shared by
    /// several groups, e.g. ``mdf["Speed", "Engine"]``.
    fn __getitem__(&self, py: Python, key: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let (name, group) = parse_lookup_key(key)?;
        self.read(py, &name, group.as_deref())
    }

    /// Build a :class:`FileLayout` describing every block in this file.
    ///
    /// Useful for debugging or analysing on-disk structure: offset, size,
    /// type, link targets and unreferenced gaps for each MDF block.
    fn file_layout(&self) -> PyResult<PyFileLayout> {
        let layout = self.mdf.file_layout()?;
        Ok(PyFileLayout { inner: layout })
    }
}

/// Streaming writer for MDF 4 files.
///
/// Build an MDF file in five logical steps:
///
/// 1. ``writer = mf4_rs.MdfWriter("out.mf4")``
/// 2. ``writer.init_mdf_file()`` — write ``##ID`` and ``##HD``.
/// 3. Define structure: :py:meth:`add_channel_group`, then any combination of
///    :py:meth:`add_time_channel`, :py:meth:`add_float_channel`,
///    :py:meth:`add_float32_channel`, :py:meth:`add_int_channel`, or the
///    generic :py:meth:`add_channel`.
/// 4. Write data: :py:meth:`start_data_block`, then either
///    :py:meth:`write_record` (one record at a time, ergonomic) or
///    :py:meth:`write_columns_f64` / :py:meth:`write_columns` (bulk numpy,
///    much faster), then :py:meth:`finish_data_block`.
/// 5. :py:meth:`finalize` — flushes and closes the file. The writer is
///    unusable after this; calling any other method raises ``MdfException``.
///
/// All identifiers (``cg_*`` for groups, ``ch_*`` for channels) are *opaque
/// strings* — pass them back to subsequent methods, but don't try to parse
/// them. Channels are auto-linked into a per-group linked list in the order
/// they were added.
///
/// Example
/// -------
/// >>> w = mf4_rs.MdfWriter("demo.mf4")
/// >>> w.init_mdf_file()
/// >>> cg = w.add_channel_group("group_0")
/// >>> t = w.add_time_channel(cg, "Time")
/// >>> y = w.add_float_channel(cg, "Speed")
/// >>> w.start_data_block(cg)
/// >>> for i in range(100):
/// ...     w.write_record(cg, [
/// ...         mf4_rs.create_float_value(i * 0.01),
/// ...         mf4_rs.create_float_value(i * 1.5),
/// ...     ])
/// >>> w.finish_data_block(cg)
/// >>> w.finalize()
#[gen_stub_pyclass]
#[pyclass(unsendable, name = "MdfWriter")]
pub struct PyMdfWriter {
    writer: Option<MdfWriter>,
    channel_groups: HashMap<String, String>, // Maps Python ID to Rust ID
    channels: HashMap<String, String>,       // Maps Python ID to Rust ID
    // Track the last channel added for each channel group (for automatic linking)
    last_channels: HashMap<String, String>,   // Maps channel group ID to last channel ID
    next_id: usize,
}

impl PyMdfWriter {
    fn add_channel_with_bits(&mut self, group_id: &str, name: &str, data_type: PyDataType, bit_count: u32) -> PyResult<String> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            let prev_channel_id = self.last_channels.get(group_id)
                .and_then(|py_id| self.channels.get(py_id))
                .cloned();
            let rust_data_type = DataType::from(data_type);
            let ch_id = writer.add_channel(cg_id, prev_channel_id.as_ref().map(|s| s.as_str()), |ch| {
                ch.data_type = rust_data_type.clone();
                ch.name = Some(name.to_string());
                ch.bit_count = bit_count;
            })?;
            let py_id = format!("ch_{}", self.next_id);
            self.next_id += 1;
            self.channels.insert(py_id.clone(), ch_id);
            self.last_channels.insert(group_id.to_string(), py_id.clone());
            Ok(py_id)
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMdfWriter {
    /// Create a writer that will produce a new MDF file at ``path``.
    ///
    /// The file is created (and truncated if it exists) but no MDF blocks
    /// are written until :py:meth:`init_mdf_file` is called.
    ///
    /// Parameters
    /// ----------
    /// path : str
    ///     Output filesystem path.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If the file cannot be created.
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let writer = MdfWriter::new(path)?;
        Ok(PyMdfWriter {
            writer: Some(writer),
            channel_groups: HashMap::new(),
            channels: HashMap::new(),
            last_channels: HashMap::new(),
            next_id: 0,
        })
    }

    /// Write the MDF identification (``##ID``) and header (``##HD``) blocks.
    ///
    /// Must be called exactly once, before adding any channel group.
    fn init_mdf_file(&mut self) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            writer.init_mdf_file()?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Append a new channel group (``##CG``) to the file.
    ///
    /// Parameters
    /// ----------
    /// name : Optional[str]
    ///     Group acquisition name. Written as a ``##TX`` block referenced
    ///     from the new ``##CG`` via ``acq_name_addr``. Pass ``None`` to
    ///     leave it unset.
    ///
    /// Returns
    /// -------
    /// str
    ///     Opaque group ID (e.g. ``"cg_0"``) — pass to subsequent
    ///     ``add_channel`` / ``write_record`` / ``finish_data_block`` calls.
    fn add_channel_group(&mut self, name: Option<String>) -> PyResult<String> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = writer.add_channel_group(None, |_cg| {})?;
            if let Some(n) = &name {
                writer.set_channel_group_name(&cg_id, n)?;
            }

            let py_id = format!("cg_{}", self.next_id);
            self.next_id += 1;
            self.channel_groups.insert(py_id.clone(), cg_id);

            Ok(py_id)
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }

    /// Attach a comment / description to a channel group.
    ///
    /// Writes a ``##TX`` block holding ``comment`` and links it from the
    /// group's ``comment_addr`` field.
    fn set_channel_group_comment(&mut self, group_id: &str, comment: &str) -> PyResult<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| MdfException::new_err("Writer has been finalized"))?;
        let rust_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| MdfException::new_err(format!("Unknown group_id: {}", group_id)))?
            .clone();
        writer.set_channel_group_comment(&rust_id, comment)?;
        Ok(())
    }
    
    /// Add a generic channel to a channel group, using the data type's
    /// natural bit width.
    ///
    /// The new channel is linked after the most recently added channel in
    /// the same group, and inherits its byte offset from the running record
    /// layout. For float channels this defaults to **32 bits** — use
    /// :py:meth:`add_float_channel` if you want 64-bit precision.
    ///
    /// Parameters
    /// ----------
    /// group_id : str
    ///     ID returned by :py:meth:`add_channel_group`.
    /// name : str
    ///     Channel name (written as a ``##TX`` block).
    /// data_type : DataType
    ///     One of the values from the ``create_data_type_*`` helpers.
    ///
    /// Returns
    /// -------
    /// str
    ///     Opaque channel ID (e.g. ``"ch_3"``).
    fn add_channel(&mut self,
                   group_id: &str,
                   name: &str,
                   data_type: PyDataType) -> PyResult<String> {
        let bits = {
            let rust_dt = DataType::from(data_type.clone());
            rust_dt.default_bits()
        };
        self.add_channel_with_bits(group_id, name, data_type, bits)
    }
    
    /// Add a 64-bit little-endian float channel and mark it as the group's
    /// master / time channel.
    ///
    /// Equivalent to :py:meth:`add_float_channel` followed by
    /// :py:meth:`set_time_channel`. Should typically be the **first** channel
    /// added to a group — subsequent channels will be linked after it.
    ///
    /// Parameters
    /// ----------
    /// group_id : str
    /// name : str
    ///     Usually ``"Time"`` or ``"t"``.
    ///
    /// Returns
    /// -------
    /// str
    ///     Channel ID.
    fn add_time_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        let ch_id = self.add_channel_with_bits(group_id, name, PyDataType { name: "FloatLE".to_string(), value: 4 }, 64)?;
        self.set_time_channel(&ch_id)?;
        Ok(ch_id)
    }

    /// Add a 64-bit little-endian float (``f64``) data channel.
    ///
    /// Recommended default for scientific / numeric data. Use
    /// :py:meth:`add_float32_channel` when 32-bit precision is enough and
    /// file size matters.
    fn add_float_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        self.add_channel_with_bits(group_id, name, PyDataType { name: "FloatLE".to_string(), value: 4 }, 64)
    }

    /// Add a 32-bit little-endian float (``f32``) data channel.
    ///
    /// Halves the per-record size compared to :py:meth:`add_float_channel`
    /// at the cost of precision (~7 decimal digits).
    fn add_float32_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        self.add_channel_with_bits(group_id, name, PyDataType { name: "FloatLE".to_string(), value: 4 }, 32)
    }

    /// Add a 64-bit little-endian unsigned integer (``u64``) data channel.
    ///
    /// For signed integers, build a generic channel with
    /// :py:meth:`add_channel` and ``create_data_type_*`` helpers (or call
    /// directly with the raw :class:`DataType`).
    fn add_int_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        self.add_channel_with_bits(group_id, name, PyDataType { name: "UnsignedIntegerLE".to_string(), value: 0 }, 64)
    }

    /// Mark an existing channel as the group's master / time channel.
    ///
    /// Sets ``channel_type = 2`` and ``sync_type = 1`` on the underlying
    /// ``##CN`` block. Most users should prefer :py:meth:`add_time_channel`,
    /// which combines channel creation with this call.
    ///
    /// Parameters
    /// ----------
    /// channel_id : str
    ///     ID returned by one of the ``add_*_channel`` methods.
    fn set_time_channel(&mut self, channel_id: &str) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            let ch_id = self.channels.get(channel_id)
                .ok_or_else(|| MdfException::new_err("Channel not found"))?;
            writer.set_time_channel(ch_id)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Open a fresh ``##DT`` data block for a channel group.
    ///
    /// Must be called once after all channels have been added, before any
    /// :py:meth:`write_record` / :py:meth:`write_columns_f64` /
    /// :py:meth:`write_columns` for that group. The writer will
    /// automatically split into multiple ``##DT`` fragments at the 4 MB
    /// boundary, joined by a ``##DL`` block when you call
    /// :py:meth:`finish_data_block`.
    fn start_data_block(&mut self, group_id: &str) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            writer.start_data_block_for_cg(cg_id, 0)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Append a single record (one sample for each channel in the group).
    ///
    /// ``values`` must contain exactly one entry per channel, in the same
    /// order channels were added — typically the master channel first, then
    /// data channels. Each value is encoded according to its channel's data
    /// type (the variant of :class:`DecodedValue` is *not* re-checked, so
    /// passing the wrong variant will produce nonsense bytes).
    ///
    /// This call has Python-level overhead per value; for bulk numeric data,
    /// prefer :py:meth:`write_columns_f64` or :py:meth:`write_columns`.
    fn write_record(&mut self, group_id: &str, values: Vec<PyDecodedValue>) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            
            let rust_values: Vec<DecodedValue> = values.into_iter().map(DecodedValue::from).collect();
            writer.write_record(cg_id, &rust_values)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Close the open ``##DT`` block for a channel group.
    ///
    /// If the group's data exceeded the 4 MB block size and was split
    /// across multiple ``##DT`` fragments, this also writes the ``##DL``
    /// (data list) block linking them together. Call once per group after
    /// all records have been written, and before :py:meth:`finalize`.
    fn finish_data_block(&mut self, group_id: &str) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            writer.finish_data_block(cg_id)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Bulk-write all channels of a group from numpy ``float64`` arrays.
    ///
    /// Equivalent to writing every record in row order, but ~10× faster than
    /// looping over :py:meth:`write_record` because it avoids per-value
    /// Python-Rust crossings and ``DecodedValue`` boxing.
    ///
    /// Parameters
    /// ----------
    /// group_id : str
    /// columns : list[numpy.ndarray]
    ///     One contiguous 1-D ``float64`` array per channel, in channel
    ///     order. **All arrays must have identical length** — that length
    ///     becomes the record count. The float values are converted to each
    ///     channel's declared data type before encoding.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If an array is not contiguous, not ``float64``, or lengths
    ///     mismatch.
    fn write_columns_f64(&mut self, _py: Python<'_>, group_id: &str, columns: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?
                .clone();

            let arrays: Vec<PyReadonlyArray1<f64>> = columns.iter()
                .map(|c| c.extract::<PyReadonlyArray1<f64>>())
                .collect::<PyResult<Vec<_>>>()?;
            let slices: Vec<&[f64]> = arrays.iter()
                .map(|a| a.as_slice().map_err(|e| MdfException::new_err(format!("Array not contiguous: {}", e))))
                .collect::<PyResult<Vec<_>>>()?;

            writer.write_columns_f64(&cg_id, &slices)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }

    /// Bulk-write all channels of a group from heterogeneous numpy arrays.
    ///
    /// Like :py:meth:`write_columns_f64` but accepts a per-column dtype, so
    /// integer / single-precision channels are written without an
    /// intermediate ``float64`` round-trip.
    ///
    /// Parameters
    /// ----------
    /// group_id : str
    /// columns : list[numpy.ndarray]
    ///     One contiguous 1-D array per channel, all of identical length.
    /// dtypes : list[str]
    ///     One entry per column, matching ``columns``. Allowed values:
    ///     ``"f64"``, ``"f32"``, ``"u64"``, ``"i64"``.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If the dtype string is not one of the supported values, the
    ///     numpy array's element type doesn't match the dtype string, the
    ///     two list lengths differ, or any array is non-contiguous.
    fn write_columns(&mut self, _py: Python<'_>, group_id: &str, columns: Vec<Bound<'_, PyAny>>, dtypes: Vec<String>) -> PyResult<()> {
        if columns.len() != dtypes.len() {
            return Err(MdfException::new_err(format!(
                "columns length ({}) must match dtypes length ({})",
                columns.len(), dtypes.len()
            )));
        }

        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?
                .clone();

            // We need to keep the extracted arrays alive for the duration of the call.
            // Use an enum to hold each typed array so lifetimes work out.
            enum OwnedArray<'py> {
                F64(PyReadonlyArray1<'py, f64>),
                F32(PyReadonlyArray1<'py, f32>),
                U64(PyReadonlyArray1<'py, u64>),
                I64(PyReadonlyArray1<'py, i64>),
            }

            let owned: Vec<OwnedArray<'_>> = columns.iter().zip(dtypes.iter())
                .map(|(col, dtype)| match dtype.as_str() {
                    "f64" => col.extract::<PyReadonlyArray1<f64>>().map(OwnedArray::F64),
                    "f32" => col.extract::<PyReadonlyArray1<f32>>().map(OwnedArray::F32),
                    "u64" => col.extract::<PyReadonlyArray1<u64>>().map(OwnedArray::U64),
                    "i64" => col.extract::<PyReadonlyArray1<i64>>().map(OwnedArray::I64),
                    other => Err(MdfException::new_err(format!(
                        "Unknown dtype '{}'; expected one of: f64, f32, u64, i64", other
                    ))),
                })
                .collect::<PyResult<Vec<_>>>()?;

            let column_data: Vec<ColumnData<'_>> = owned.iter()
                .map(|arr| match arr {
                    OwnedArray::F64(a) => a.as_slice()
                        .map(ColumnData::F64)
                        .map_err(|e| MdfException::new_err(format!("Array not contiguous: {}", e))),
                    OwnedArray::F32(a) => a.as_slice()
                        .map(ColumnData::F32)
                        .map_err(|e| MdfException::new_err(format!("Array not contiguous: {}", e))),
                    OwnedArray::U64(a) => a.as_slice()
                        .map(ColumnData::U64)
                        .map_err(|e| MdfException::new_err(format!("Array not contiguous: {}", e))),
                    OwnedArray::I64(a) => a.as_slice()
                        .map(ColumnData::I64)
                        .map_err(|e| MdfException::new_err(format!("Array not contiguous: {}", e))),
                })
                .collect::<PyResult<Vec<_>>>()?;

            writer.write_columns(&cg_id, &column_data)?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }

    /// Flush all buffered bytes to disk and close the file.
    ///
    /// After this returns, the writer is consumed: any subsequent method
    /// call raises ``MdfException``. Calling :py:meth:`finalize` more than
    /// once also raises.
    fn finalize(&mut self) -> PyResult<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize()?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer already finalized"))
        }
    }
}

/// Lightweight, self-contained index over an MDF 4 file.
///
/// An index records the byte ranges of each channel, fully resolves every
/// conversion block, and serialises to JSON. Navigate it by **name**
/// (:py:attr:`groups`, :py:meth:`group`, :py:meth:`channel`). It also remembers
/// its data :py:attr:`source` (file path or URL); reading is lazy — the
/// byte-range request happens on :py:meth:`read` / :py:meth:`values`, never at
/// build time.
///
/// **Limitation:** compressed (``##DZ``) data blocks are not supported.
///
/// Example
/// -------
/// >>> idx = mf4_rs.MdfIndex.from_url("https://host/recording.mf4")  # only metadata fetched
/// >>> speed = idx.read("Speed")            # pandas Series; range request happens now
/// >>> rpm   = idx.values("RPM")            # numpy float64, no timestamp index
/// >>> idx.save("recording.idx.json")
/// >>> idx = mf4_rs.MdfIndex.load("recording.idx.json")
/// >>> idx.source = "recording.mf4"         # re-attach a source after load
/// >>> t = idx.read("Speed")
#[gen_stub_pyclass]
#[pyclass(name = "MdfIndex")]
pub struct PyMdfIndex {
    index: MdfIndex,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMdfIndex {
    /// Build a fresh index by parsing an MDF file from disk.
    ///
    /// All conversions are resolved during construction so the index is fully
    /// self-contained afterwards.
    ///
    /// Parameters
    /// ----------
    /// path : str
    ///     Path to a ``.mf4`` file.
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::from_file(path)? })
    }

    /// Load a previously saved JSON index (companion to :py:meth:`save`).
    ///
    /// The original MDF file is only needed later, when you actually read
    /// values via :py:meth:`open`.
    #[staticmethod]
    fn load(path: &str) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::load_from_file(path)? })
    }

    /// Build an index from an MDF file served over HTTP / S3 using range
    /// requests, without downloading the whole file.
    ///
    /// Issues range reads only for metadata blocks; sample data is never
    /// fetched. With the default 1 MiB read-ahead chunk a typical file
    /// collapses to a handful of HTTP requests regardless of size.
    ///
    /// Parameters
    /// ----------
    /// url : str
    ///     ``http://`` / ``https://`` URL honouring ``Range`` requests.
    /// chunk_size : int, optional
    ///     Metadata read-ahead chunk size in bytes (default 1 MiB).
    #[staticmethod]
    fn from_url(py: Python, url: &str, chunk_size: Option<u64>) -> PyResult<Self> {
        let url = url.to_string();
        let chunk = chunk_size.unwrap_or(1 << 20);
        // The URL is remembered as the index's source; only metadata is fetched
        // here — sample data is range-requested lazily on `read` / `values`.
        let index = py.allow_threads(move || -> Result<MdfIndex, MdfError> {
            MdfIndex::from_url_with_chunk_size(&url, chunk)
        })?;
        Ok(PyMdfIndex { index })
    }

    /// Serialize the index to JSON at ``path`` (dependency-free).
    fn save(&self, path: &str) -> PyResult<()> {
        self.index.save_to_file(path)?;
        Ok(())
    }

    /// Metadata for every channel group, in file order.
    ///
    /// Each :class:`GroupInfo` carries its ``channels`` list, so the whole
    /// structure is available from a single ``index.groups`` access.
    #[getter]
    fn groups(&self) -> Vec<PyChannelGroupInfo> {
        self.index.groups().iter().map(PyChannelGroupInfo::from_indexed).collect()
    }

    /// Find a channel group by name (first match), or ``None``.
    fn group(&self, name: &str) -> Option<PyChannelGroupInfo> {
        self.index.group(name).map(PyChannelGroupInfo::from_indexed)
    }

    /// Find a channel by name across all groups (first match), or ``None``.
    fn channel(&self, name: &str) -> Option<PyChannelInfo> {
        self.index.channel(name).map(PyChannelInfo::from_indexed)
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[getter]
    fn channel_names(&self) -> Vec<String> {
        self.index.channel_names().into_iter().map(String::from).collect()
    }

    /// Names of the groups that contain a channel called ``name``.
    ///
    /// Use this to disambiguate a channel name shared by several groups, then
    /// pass the chosen group to :py:meth:`read` / :py:meth:`values`.
    fn groups_with_channel(&self, name: &str) -> Vec<String> {
        self.index
            .find_channels(name)
            .into_iter()
            .filter_map(|(g, _)| self.index.groups().get(g).and_then(|grp| grp.name.clone()))
            .collect()
    }

    /// Byte ranges ``[(offset, length), ...]`` occupied by a channel.
    ///
    /// Accounts for the channel's record-layout position and data-block
    /// splitting. Power-user entry point for issuing your own partial reads.
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    ///     Disambiguate by group when the name is not unique.
    fn byte_ranges(&self, name: &str, group: Option<&str>) -> PyResult<Vec<(u64, u64)>> {
        Ok(match group {
            Some(g) => self.index.byte_ranges_in(g, name)?,
            None => self.index.byte_ranges(name)?,
        })
    }

    /// Byte ranges covering a record window ``[start, start+count)``.
    fn byte_ranges_for_records(
        &self,
        name: &str,
        start_record: u64,
        record_count: u64,
    ) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.byte_ranges_for_records(name, start_record, record_count)?)
    }

    /// Inspect the conversion attached to a channel (by name).
    ///
    /// Returns ``None`` if the channel has no conversion, otherwise a dict
    /// describing it (``conversion_type``, ``values``, ``resolved_texts``,
    /// ``formula`` …).
    fn conversion_info(&self, name: &str) -> PyResult<Option<HashMap<String, PyObject>>> {
        let (g, c) = self.index.locate(name).ok_or_else(|| {
            MdfException::new_err(format!("Channel '{}' not found", name))
        })?;
        let channel = &self.index.groups()[g].channels[c];

        if let Some(conversion) = &channel.conversion {
            Python::with_gil(|py| {
                let mut info = HashMap::new();
                info.insert("conversion_type".to_string(), format!("{:?}", conversion.cc_type).to_object(py));
                info.insert("precision".to_string(), conversion.cc_precision.to_object(py));
                info.insert("flags".to_string(), conversion.cc_flags.to_object(py));
                info.insert("values_count".to_string(), conversion.cc_val_count.to_object(py));
                info.insert("values".to_string(), conversion.cc_val.to_object(py));
                if let Some(resolved_texts) = &conversion.resolved_texts {
                    let texts: HashMap<usize, String> = resolved_texts.clone();
                    info.insert("resolved_texts".to_string(), texts.to_object(py));
                }
                if conversion.resolved_conversions.is_some() {
                    info.insert("has_resolved_conversions".to_string(), true.to_object(py));
                }
                if let Some(formula) = &conversion.formula {
                    info.insert("formula".to_string(), formula.to_object(py));
                }
                Ok(Some(info))
            })
        } else {
            Ok(None)
        }
    }

    /// Total size of the source MDF file when the index was built.
    #[getter]
    fn file_size(&self) -> u64 {
        self.index.file_size
    }

    /// The data source attached to this index (file path or URL), or ``None``.
    ///
    /// Set automatically by :py:meth:`from_file` / :py:meth:`from_url`. After
    /// :py:meth:`load` re-attach one by assigning this property (or calling
    /// :py:meth:`set_source`). Building the index never reads sample data; the
    /// source is only range-requested when you call :py:meth:`read` / :py:meth:`values`.
    #[getter]
    fn source(&self) -> Option<String> {
        self.index.source_string()
    }

    /// A flat catalog of every channel as ``(source, group, channel)`` tuples.
    ///
    /// ``source`` is this index's attached source (file path or URL) — the same
    /// for every row, so catalogs from several indexes concatenate cleanly.
    /// Built from metadata only; no sample data is read (cheap even for a
    /// URL-backed index). ``group`` / ``channel`` are ``None`` if unnamed.
    ///
    /// Returns
    /// -------
    /// list[tuple[Optional[str], Optional[str], Optional[str]]]
    fn list_signals(&self) -> Vec<(Option<String>, Option<String>, Option<String>)> {
        self.index.signal_list()
    }

    #[setter(source)]
    fn set_source_prop(&mut self, value: Option<&str>) {
        self.apply_source(value);
    }

    /// Attach (or clear) the data source used by :py:meth:`read` / :py:meth:`values`.
    ///
    /// A value starting with ``http://`` or ``https://`` is treated as a URL;
    /// anything else as a local file path. Pass ``None`` to clear.
    fn set_source(&mut self, value: Option<&str>) {
        self.apply_source(value);
    }

    /// Read a channel as a ``pandas.Series`` of values indexed by timestamps.
    ///
    /// **Lazy:** the byte-range request to the attached source happens now, not
    /// when the index was built. Values carry all conversions; the index is the
    /// group master converted to a ``DatetimeIndex`` (or raw seconds / default
    /// index when there is no start time / master).
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    ///     Disambiguate by group when the channel name is not unique.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If no source is attached, the channel is missing, or pandas is absent.
    fn read(&self, py: Python, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        let pd = check_pandas_available(py)?;
        // Release the GIL during the (potentially blocking, e.g. HTTP) read.
        let signal = py.allow_threads(|| match group {
            Some(g) => self.index.read_in(g, name),
            None => self.index.read(name),
        })?;
        signal_to_series(
            py, &pd, &signal.name, &signal.timestamps, signal.values, self.index.start_time_ns,
        )
    }

    /// Read a numeric channel by name as a plain numpy ``float64`` array.
    ///
    /// Lazy fast path — just the values, no timestamp index, pandas-free.
    /// Invalid / non-numeric samples are ``NaN``.
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    fn values<'py>(&self, py: Python<'py>, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        let (g, c) = match group {
            Some(gn) => self.index.locate_in(gn, name).ok_or_else(|| {
                MdfException::new_err(format!("Channel '{}' not found in group '{}'", name, gn))
            })?,
            None => self.index.locate(name).ok_or_else(|| {
                MdfException::new_err(format!("Channel '{}' not found", name))
            })?,
        };
        // Release the GIL during the (potentially blocking, e.g. HTTP) read.
        let values = py.allow_threads(|| self.index.read_values_f64_via_source(g, c))?;
        Ok(PyArray1::from_vec_bound(py, values).into())
    }

    /// ``index["Speed"]`` — shorthand for :py:meth:`read` (timestamp-indexed Series).
    ///
    /// Pass a ``(name, group)`` tuple to disambiguate a channel name shared by
    /// several groups, e.g. ``index["Speed", "Engine"]``.
    fn __getitem__(&self, py: Python, key: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let (name, group) = parse_lookup_key(key)?;
        self.read(py, &name, group.as_deref())
    }
}

impl PyMdfIndex {
    /// Apply a string source, auto-detecting URL vs file path.
    fn apply_source(&mut self, value: Option<&str>) {
        match value {
            None => self.index.source = None,
            Some(v) if v.starts_with("http://") || v.starts_with("https://") => {
                self.index.set_url(v);
            }
            Some(v) => self.index.set_file(v),
        }
    }
}

// ---------------------------------------------------------------------------
// Block layout visualization
// ---------------------------------------------------------------------------

/// A single outbound link from a block to another block.
///
/// Attributes
/// ----------
/// name : str
///     Link's role inside the source block (e.g. ``"first_dg_addr"``).
/// target : int
///     Absolute file offset of the target block (0 means null link).
/// target_type : Optional[str]
///     Block type at the target offset, when known (e.g. ``"##DG"``).
#[gen_stub_pyclass]
#[pyclass(name = "LinkInfo")]
#[derive(Clone)]
pub struct PyLinkInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub target: u64,
    #[pyo3(get)]
    pub target_type: Option<String>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyLinkInfo {
    fn __repr__(&self) -> String {
        if self.target == 0 {
            format!("Link({} -> null)", self.name)
        } else {
            format!(
                "Link({} -> 0x{:010x} [{}])",
                self.name,
                self.target,
                self.target_type.as_deref().unwrap_or("?")
            )
        }
    }
}

impl From<LinkInfo> for PyLinkInfo {
    fn from(l: LinkInfo) -> Self {
        PyLinkInfo {
            name: l.name,
            target: l.target,
            target_type: l.target_type,
        }
    }
}

/// A single MDF block discovered while walking the file's link graph.
///
/// Attributes
/// ----------
/// offset, end_offset, size : int
///     On-disk byte range of the block.
/// block_type : str
///     Four-character MDF block tag (e.g. ``"##HD"``, ``"##CN"``).
/// description : str
///     Short human-readable summary (e.g. ``"channel 'Speed' [f64@0..8]"``).
/// links : list[LinkInfo]
///     Outbound links to other blocks.
/// extra : Optional[str]
///     Block-type-specific extra information, when applicable.
#[gen_stub_pyclass]
#[pyclass(name = "BlockInfo")]
#[derive(Clone)]
pub struct PyBlockInfo {
    #[pyo3(get)]
    pub offset: u64,
    #[pyo3(get)]
    pub end_offset: u64,
    #[pyo3(get)]
    pub size: u64,
    #[pyo3(get)]
    pub block_type: String,
    #[pyo3(get)]
    pub description: String,
    #[pyo3(get)]
    pub links: Vec<PyLinkInfo>,
    #[pyo3(get)]
    pub extra: Option<String>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyBlockInfo {
    fn __repr__(&self) -> String {
        format!(
            "Block({} @ 0x{:010x}..0x{:010x}, size={}, {})",
            self.block_type, self.offset, self.end_offset, self.size, self.description
        )
    }
}

impl From<BlockInfo> for PyBlockInfo {
    fn from(b: BlockInfo) -> Self {
        PyBlockInfo {
            offset: b.offset,
            end_offset: b.end_offset,
            size: b.size,
            block_type: b.block_type,
            description: b.description,
            links: b.links.into_iter().map(Into::into).collect(),
            extra: b.extra,
        }
    }
}

/// A range of bytes in the file not covered by any visited block.
///
/// Gaps usually represent alignment padding, abandoned scratch space, or
/// (rarely) blocks that aren't reachable from the link graph.
///
/// Attributes
/// ----------
/// start, end, size : int
#[gen_stub_pyclass]
#[pyclass(name = "GapInfo")]
#[derive(Clone)]
pub struct PyGapInfo {
    #[pyo3(get)]
    pub start: u64,
    #[pyo3(get)]
    pub end: u64,
    #[pyo3(get)]
    pub size: u64,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyGapInfo {
    fn __repr__(&self) -> String {
        format!(
            "Gap(0x{:010x}..0x{:010x}, size={})",
            self.start, self.end, self.size
        )
    }
}

impl From<GapInfo> for PyGapInfo {
    fn from(g: GapInfo) -> Self {
        PyGapInfo {
            start: g.start,
            end: g.end,
            size: g.size,
        }
    }
}

/// Full structural layout of an MDF file: blocks, links, and gaps.
///
/// Build one with :py:meth:`from_file` or :py:meth:`Mdf.file_layout`. Use
/// it to debug file structure, audit storage efficiency, or render a
/// human-readable map of an MDF.
#[gen_stub_pyclass]
#[pyclass(name = "FileLayout")]
pub struct PyFileLayout {
    pub(crate) inner: FileLayout,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyFileLayout {
    /// Build a layout by parsing an MDF file from disk.
    ///
    /// Parameters
    /// ----------
    /// path : str
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        let inner = FileLayout::from_file(path)?;
        Ok(PyFileLayout { inner })
    }

    /// Total size of the file in bytes.
    #[getter]
    fn file_size(&self) -> u64 {
        self.inner.file_size
    }

    /// List of all discovered blocks, sorted by offset (``list[BlockInfo]``).
    #[getter]
    fn blocks(&self) -> Vec<PyBlockInfo> {
        self.inner.blocks.iter().cloned().map(Into::into).collect()
    }

    /// Byte ranges not covered by any visited block (``list[GapInfo]``).
    #[getter]
    fn gaps(&self) -> Vec<PyGapInfo> {
        self.inner.gaps.iter().cloned().map(Into::into).collect()
    }

    /// Render the layout as a flat, sorted text listing.
    ///
    /// Each line is ``offset .. end_offset  block_type  description``,
    /// including link targets — easy to ``grep`` / diff.
    fn to_text(&self) -> String {
        self.inner.to_text()
    }

    /// Render the link graph as an indented tree rooted at ``##ID``.
    fn to_tree(&self) -> String {
        self.inner.to_tree()
    }

    /// Serialize the layout to a pretty-printed JSON string.
    fn to_json(&self) -> PyResult<String> {
        Ok(self.inner.to_json()?)
    }

    /// Write the flat text listing (see :py:meth:`to_text`) to ``path``.
    fn write_text_to_file(&self, path: &str) -> PyResult<()> {
        self.inner.write_text_to_file(path)?;
        Ok(())
    }

    /// Write the indented tree view (see :py:meth:`to_tree`) to ``path``.
    fn write_tree_to_file(&self, path: &str) -> PyResult<()> {
        self.inner.write_tree_to_file(path)?;
        Ok(())
    }

    /// Write the JSON representation (see :py:meth:`to_json`) to ``path``.
    fn write_json_to_file(&self, path: &str) -> PyResult<()> {
        self.inner.write_json_to_file(path)?;
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!(
            "FileLayout(file_size={}, blocks={}, gaps={})",
            self.inner.file_size,
            self.inner.blocks.len(),
            self.inner.gaps.len()
        )
    }
}

/// Build a :class:`FileLayout` from an MDF file on disk.
///
/// Functional alias for :py:meth:`FileLayout.from_file`.
///
/// Parameters
/// ----------
/// path : str
///
/// Returns
/// -------
/// FileLayout
#[gen_stub_pyfunction]
#[pyfunction]
fn file_layout_from_file(path: &str) -> PyResult<PyFileLayout> {
    PyFileLayout::from_file(path)
}

/// Cut an MDF file by time, copying only records whose master channel value
/// falls within the inclusive `[start_time, end_time]` window.
///
/// The output preserves fixed-length numeric, string, and byte-array
/// channels, per-record invalidation bytes, and VLSD ("signal-based")
/// channels (a fresh ##SD chain is written for each kept VLSD channel).
/// Per-channel conversion / source / metadata blocks are not re-emitted, so
/// the output channels read as raw values.
///
/// Parameters
/// ----------
/// input_path : str
///     Path to the source MF4 file.
/// output_path : str
///     Destination path for the trimmed file.
/// start_time : float
///     Start of the window in seconds (inclusive).
/// end_time : float
///     End of the window in seconds (inclusive).
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (input_path, output_path, start_time, end_time))]
fn cut_mdf_by_time(
    input_path: &str,
    output_path: &str,
    start_time: f64,
    end_time: f64,
) -> PyResult<()> {
    crate::cut::cut_mdf_by_time(input_path, output_path, start_time, end_time)?;
    Ok(())
}

/// Convert a Python `datetime`, an ISO 8601 string, or a numeric UNIX
/// timestamp (seconds, with fractional part) into UNIX-epoch nanoseconds.
fn coerce_to_unix_ns(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<i64> {
    let datetime_mod = py.import_bound("datetime")?;
    let datetime_cls = datetime_mod.getattr("datetime")?;

    // datetime.datetime — call .timestamp() to get seconds since epoch.
    if value.is_instance(&datetime_cls)? {
        let dt = ensure_utc_aware(py, value, &datetime_mod)?;
        let ts: f64 = dt.call_method0("timestamp")?.extract()?;
        return Ok((ts * 1.0e9).round() as i64);
    }

    // String: try datetime.fromisoformat (Python ≥ 3.7).
    if let Ok(s) = value.extract::<String>() {
        // datetime.fromisoformat in Python 3.11+ accepts trailing 'Z'; for
        // earlier versions we normalise it to '+00:00' before parsing.
        let normalized = if s.ends_with('Z') || s.ends_with('z') {
            format!("{}+00:00", &s[..s.len() - 1])
        } else {
            s
        };
        let parsed = datetime_cls.call_method1("fromisoformat", (normalized,))?;
        let dt = ensure_utc_aware(py, &parsed, &datetime_mod)?;
        let ts: f64 = dt.call_method0("timestamp")?.extract()?;
        return Ok((ts * 1.0e9).round() as i64);
    }

    // Numeric timestamp (int or float seconds since epoch). Tried last so a
    // `datetime` (which is also coercible to a number via __float__ in some
    // shims) hits the dedicated path above.
    if let Ok(secs) = value.extract::<f64>() {
        if !secs.is_finite() {
            return Err(MdfException::new_err("timestamp is not a finite number"));
        }
        return Ok((secs * 1.0e9).round() as i64);
    }

    Err(MdfException::new_err(
        "expected datetime.datetime, ISO 8601 string, or numeric UNIX timestamp",
    ))
}

/// If `dt` is timezone-naive, attach UTC; otherwise return it unchanged.
fn ensure_utc_aware<'py>(
    py: Python<'py>,
    dt: &Bound<'py, PyAny>,
    datetime_mod: &Bound<'py, PyModule>,
) -> PyResult<Bound<'py, PyAny>> {
    let tzinfo = dt.getattr("tzinfo")?;
    if !tzinfo.is_none() {
        return Ok(dt.clone());
    }
    let timezone = datetime_mod.getattr("timezone")?;
    let utc = timezone.getattr("utc")?;
    let kwargs = [("tzinfo", utc)].into_py_dict_bound(py);
    dt.call_method("replace", (), Some(&kwargs))
}

/// Cut an MDF file by absolute UTC time. Accepts ISO 8601 strings (e.g.
/// `"2024-01-15T12:34:56Z"`), `datetime.datetime` objects (naive values are
/// assumed to be UTC), or numeric UNIX timestamps in seconds.
///
/// The window is inclusive on both ends. The source file must record a
/// non-zero `HD.abs_time` (set by the writer / asammdf when creating the
/// file) — without it the relative offset cannot be computed and an
/// `MdfException` is raised.
///
/// Other behaviour matches [`cut_mdf_by_time`]: VLSD payloads, byte-array
/// channels, and per-record invalidation bytes are preserved verbatim.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (input_path, output_path, start_utc, end_utc))]
fn cut_mdf_by_utc(
    py: Python<'_>,
    input_path: &str,
    output_path: &str,
    start_utc: &Bound<'_, PyAny>,
    end_utc: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let start_ns = coerce_to_unix_ns(py, start_utc)?;
    let end_ns = coerce_to_unix_ns(py, end_utc)?;
    crate::cut::cut_mdf_by_utc_ns(input_path, output_path, start_ns, end_ns)?;
    Ok(())
}

// Helper functions

/// Wrap a Python ``float`` in a :class:`DecodedValue` of variant ``Float``.
///
/// Use this to feed values into :py:meth:`MdfWriter.write_record` for any
/// floating-point channel (32- or 64-bit; the value is truncated to the
/// channel's declared bit width on encode).
#[gen_stub_pyfunction]
#[pyfunction]
fn create_float_value(value: f64) -> PyDecodedValue {
    PyDecodedValue::Float { value }
}

/// Wrap a Python ``int`` in a ``UnsignedInteger`` :class:`DecodedValue`.
///
/// Suitable for any unsigned integer channel; the value is truncated to the
/// channel's bit width on encode.
#[gen_stub_pyfunction]
#[pyfunction]
fn create_uint_value(value: u64) -> PyDecodedValue {
    PyDecodedValue::UnsignedInteger { value }
}

/// Wrap a Python ``int`` in a ``SignedInteger`` :class:`DecodedValue`.
#[gen_stub_pyfunction]
#[pyfunction]
fn create_int_value(value: i64) -> PyDecodedValue {
    PyDecodedValue::SignedInteger { value }
}

/// Wrap a Python ``str`` in a ``String`` :class:`DecodedValue`.
///
/// For string channels (``StringUtf8`` etc.). Encoding into the file is
/// done according to the channel's declared string data type.
#[gen_stub_pyfunction]
#[pyfunction]
fn create_string_value(value: String) -> PyDecodedValue {
    PyDecodedValue::String { value }
}

/// Return the :class:`DataType` for little-endian unsigned integers.
///
/// Pair with :py:meth:`MdfWriter.add_channel` when you need an unsigned
/// integer channel of non-default width (otherwise see
/// :py:meth:`MdfWriter.add_int_channel`).
#[gen_stub_pyfunction]
#[pyfunction]
fn create_data_type_uint_le() -> PyDataType {
    PyDataType { name: "UnsignedIntegerLE".to_string(), value: 0 }
}

/// Return the :class:`DataType` for little-endian IEEE-754 floats.
///
/// Pair with :py:meth:`MdfWriter.add_channel`. Defaults to 32 bits when
/// passed to ``add_channel`` — for f64 use :py:meth:`MdfWriter.add_float_channel`.
#[gen_stub_pyfunction]
#[pyfunction]
fn create_data_type_float_le() -> PyDataType {
    PyDataType { name: "FloatLE".to_string(), value: 4 }
}

/// Return the :class:`DataType` for UTF-8 encoded string channels.
#[gen_stub_pyfunction]
#[pyfunction]
fn create_data_type_string_utf8() -> PyDataType {
    PyDataType { name: "StringUtf8".to_string(), value: 7 }
}

/// Merge two MDF files into a new file at ``output``.
///
/// Channel groups whose layouts (record-id length and channel list — names,
/// data types, bit/byte offsets, bit count, channel type, VLSD flag) match
/// are concatenated; non-matching groups are appended as separate groups.
/// Supports normal numeric, fixed ``ByteArray``, and VLSD signal channels
/// (variable-length strings and byte arrays stored in ``##SD`` blocks).
///
/// Parameters
/// ----------
/// output : str
///     Destination path for the merged file.
/// first, second : str
///     Source file paths. Must be uncompressed MDF 4.10+ files.
#[gen_stub_pyfunction]
#[pyfunction]
fn merge_files(output: &str, first: &str, second: &str) -> PyResult<()> {
    crate::merge::merge_files(output, first, second)?;
    Ok(())
}

/// The main Python module initialization function
pub fn init_mf4_rs_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let mdf_exception = m.py().get_type_bound::<MdfException>();
    mdf_exception.setattr(
        "__doc__",
        "Exception raised by mf4_rs for any error originating from the \
         underlying Rust library: malformed or unsupported MDF files, \
         out-of-range indices, missing channels, I/O failures, conversion \
         dependency cycles, attempts to use a finalized writer, missing \
         optional dependencies (e.g. pandas), and so on.\n\n\
         All public methods on PyMDF, PyMdfWriter, PyMdfIndex, and the \
         module-level free functions raise this (or a subclass of \
         Exception) on failure. Catch it as a single category to handle \
         every mf4_rs failure path:\n\n\
         >>> try:\n\
         ...     mdf = mf4_rs.PyMDF(\"missing.mf4\")\n\
         ... except mf4_rs.MdfException as e:\n\
         ...     print(\"could not open:\", e)",
    )?;
    m.add("MdfException", mdf_exception)?;
    
    // Classes
    m.add_class::<PyMDF>()?;
    m.add_class::<PyMdfWriter>()?;
    m.add_class::<PyMdfIndex>()?;
    m.add_class::<PyChannelGroupInfo>()?;
    m.add_class::<PyChannelInfo>()?;
    m.add_class::<PyDecodedValue>()?;
    m.add_class::<PyDataType>()?;
    m.add_class::<PyFileLayout>()?;
    m.add_class::<PyBlockInfo>()?;
    m.add_class::<PyLinkInfo>()?;
    m.add_class::<PyGapInfo>()?;

    // Helper functions
    m.add_function(wrap_pyfunction!(create_float_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_uint_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_int_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_string_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_uint_le, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_float_le, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_string_utf8, m)?)?;
    m.add_function(wrap_pyfunction!(file_layout_from_file, m)?)?;
    m.add_function(wrap_pyfunction!(merge_files, m)?)?;
    m.add_function(wrap_pyfunction!(cut_mdf_by_time, m)?)?;
    m.add_function(wrap_pyfunction!(cut_mdf_by_utc, m)?)?;

    Ok(())
}

// Generates `pub fn stub_info() -> pyo3_stub_gen::Result<StubInfo>`, used by
// `src/bin/stub_gen.rs` to walk the gathered `#[gen_stub_*]` declarations and
// emit the .pyi file. No runtime cost — the gathering only runs when the
// stub-gen binary is invoked.
define_stub_info_gatherer!(stub_info);
