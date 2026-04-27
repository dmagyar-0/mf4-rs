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
use std::collections::HashMap;

use crate::api::mdf::MDF;
use crate::writer::{MdfWriter, ColumnData};
use crate::index::{MdfIndex, FileRangeReader, IndexedChannel};
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
#[pyclass]
#[derive(Debug, Clone)]
pub struct PyDataType {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub value: u8,
}

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
/// :py:meth:`PyMdfWriter.write_record`. When *reading*, most APIs return native
/// Python objects (``float`` / ``int`` / ``str`` / ``bytes``) directly, so you
/// usually only construct ``PyDecodedValue`` to feed back into the writer.
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
#[pyclass]
#[derive(Debug, Clone)]
pub enum PyDecodedValue {
    Float { value: f64 },
    UnsignedInteger { value: u64 },
    SignedInteger { value: i64 },
    String { value: String },
    ByteArray { value: Vec<u8> },
    Unknown { },
}

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
/// This is more efficient than creating a PyDecodedValue and immediately extracting its value.
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
/// Returned by :py:meth:`PyMDF.get_all_channels`,
/// :py:meth:`PyMDF.get_channels_for_group`, and
/// :py:meth:`PyMdfIndex.get_channel_info_by_name`.
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
/// data_type : PyDataType
///     The MDF data type of the raw samples.
/// bit_count : int
///     Width of the raw value in bits (e.g. 32 for f32, 64 for f64/u64).
#[pyclass]
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
}

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

/// Read-only metadata describing a channel group (``##CG`` block).
///
/// Returned by :py:meth:`PyMDF.channel_groups`.
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
#[pyclass]
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
}

#[pymethods]
impl PyChannelGroupInfo {
    fn __str__(&self) -> String {
        format!("ChannelGroup(name={:?}, channels={}, records={})", 
                self.name, self.channel_count, self.record_count)
    }
    
    fn __repr__(&self) -> String {
        self.__str__()
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

/// Helper function to find the master/time channel in a channel group
/// Returns None if no master channel is found
fn find_master_channel<'a>(channels: &Vec<crate::api::channel::Channel<'a>>) -> Option<usize> {
    for (idx, channel) in channels.iter().enumerate() {
        let block = channel.block();
        // Master channels have channel_type == 2
        if block.channel_type == 2 {
            return Some(idx);
        }
    }
    None
}

/// Helper function to find the master/time channel in indexed channels
/// Returns None if no master channel is found
fn find_master_channel_indexed(channels: &Vec<IndexedChannel>) -> Option<usize> {
    for (idx, channel) in channels.iter().enumerate() {
        // Master channels have channel_type == 2
        if channel.channel_type == 2 {
            return Some(idx);
        }
    }
    None
}

/// Helper function to create a pandas DatetimeIndex from relative time values
///
/// # Arguments
/// * `py` - Python GIL token
/// * `pd` - pandas module PyObject
/// * `relative_times` - Vector of relative time values (in seconds) as PyObjects
/// * `start_ns` - Start time in nanoseconds since epoch
///
/// # Returns
/// PyObject representing a pandas DatetimeIndex, or an error if conversion fails
///
/// # Error Handling
/// This function handles multiple edge cases:
/// - None values in master channel (converted to NaT - Not a Time)
/// - Integer time values (i64, u64) in addition to float values
/// - Negative time values that would create timestamps before epoch
/// - Overflow/underflow when adding large time deltas
fn create_datetime_index(
    py: Python,
    pd: &PyObject,
    relative_times: &[PyObject],
    start_ns: u64,
) -> PyResult<PyObject> {
    // Convert start time from nanoseconds to a pandas Timestamp
    let to_datetime = pd.getattr(py, "to_datetime")?;
    let start_timestamp = to_datetime.call(
        py,
        (start_ns,),
        Some([("unit", "ns")].into_py_dict(py))
    )?;

    // Create a Timedelta for each relative time and add to start time
    let timedelta_class = pd.getattr(py, "Timedelta")?;
    let nat = pd.getattr(py, "NaT")?;  // Not-a-Time for None values
    let mut absolute_times = Vec::with_capacity(relative_times.len());

    for rel_time in relative_times {
        // Handle None values (invalid samples) - convert to NaT
        if rel_time.is_none(py) {
            absolute_times.push(nat.clone());
            continue;
        }

        // Try to extract the time value as f64, then i64, then u64
        // Master channels can be float or integer type
        let seconds: f64 = if let Ok(f) = rel_time.extract::<f64>(py) {
            f
        } else if let Ok(i) = rel_time.extract::<i64>(py) {
            i as f64
        } else if let Ok(u) = rel_time.extract::<u64>(py) {
            u as f64
        } else {
            // If we can't extract as any numeric type, use NaT
            absolute_times.push(nat.clone());
            continue;
        };

        // Check for edge cases that might cause overflow or invalid timestamps
        // pandas datetime64[ns] has range: 1678-2262 (approximately)
        // If start_ns + seconds would overflow, pandas will raise an OutOfBoundsDatetime error
        // We'll let pandas handle this and propagate the error up

        // Create a Timedelta in seconds and add to start time
        // This can fail if the resulting timestamp is out of pandas' valid range
        let delta = timedelta_class.call(
            py,
            (seconds,),
            Some([("unit", "s")].into_py_dict(py))
        )?;

        let absolute_time = start_timestamp.call_method1(py, "__add__", (delta,))?;
        absolute_times.push(absolute_time);
    }

    // Create DatetimeIndex from the list of timestamps
    let datetime_index_class = pd.getattr(py, "DatetimeIndex")?;
    let datetime_index = datetime_index_class.call1(py, (absolute_times,))?;

    Ok(datetime_index)
}

/// Read-only handle to an MDF 4 file, backed by a memory-mapped buffer.
///
/// Opens the file lazily: metadata (block tree, channel names, conversions)
/// is parsed up front, but sample data is only decoded when you call one of
/// the ``get_channel_*`` methods. The mmap stays alive for the lifetime of
/// the ``PyMDF`` instance.
///
/// Example
/// -------
/// >>> import mf4_rs
/// >>> mdf = mf4_rs.PyMDF("recording.mf4")
/// >>> for group in mdf.channel_groups():
/// ...     print(group.name, group.record_count)
/// >>> values = mdf.get_channel_values("Temperature")  # numpy.ndarray
#[pyclass]
pub struct PyMDF {
    mdf: Box<MDF>,
}

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
        Ok(PyMDF { mdf })
    }

    /// List metadata for every channel group in the file.
    ///
    /// Returns
    /// -------
    /// list[PyChannelGroupInfo]
    ///     One entry per ``##CG`` block, in file order.
    fn channel_groups(&self) -> PyResult<Vec<PyChannelGroupInfo>> {
        let mut py_groups = Vec::new();
        
        for group in self.mdf.channel_groups() {
            let name = group.name()?;
            let comment = group.comment()?;
            let channel_count = group.channels().len();
            let record_count = group.raw_channel_group().block.cycles_nr;
            
            py_groups.push(PyChannelGroupInfo {
                name,
                comment,
                channel_count,
                record_count,
            });
        }
        
        Ok(py_groups)
    }
    
    /// Return metadata for every channel across every group.
    ///
    /// Returns
    /// -------
    /// list[PyChannelInfo]
    ///     Channels are emitted in file order, group-by-group. Channels with
    ///     duplicate names (common across groups) appear multiple times — use
    ///     :py:meth:`get_channels_for_group` if you need per-group context.
    fn get_all_channels(&self) -> PyResult<Vec<PyChannelInfo>> {
        let mut channels = Vec::new();
        
        for group in self.mdf.channel_groups() {
            for channel in group.channels() {
                let block = channel.block();
                let info = PyChannelInfo {
                    name: channel.name()?,
                    unit: channel.unit()?,
                    comment: channel.comment()?,
                    data_type: PyDataType::from(&block.data_type),
                    bit_count: block.bit_count,
                };
                channels.push(info);
            }
        }
        
        Ok(channels)
    }
    
    /// Return metadata for every channel in a single group.
    ///
    /// Parameters
    /// ----------
    /// group_index : int
    ///     Zero-based index into :py:meth:`channel_groups`.
    ///
    /// Returns
    /// -------
    /// list[PyChannelInfo]
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If ``group_index`` is out of bounds.
    fn get_channels_for_group(&self, group_index: usize) -> PyResult<Vec<PyChannelInfo>> {
        let groups: Vec<_> = self.mdf.channel_groups().into_iter().collect();
        if let Some(group) = groups.get(group_index) {
            let mut channels = Vec::new();
            
            for channel in group.channels() {
                let block = channel.block();
                let info = PyChannelInfo {
                    name: channel.name()?,
                    unit: channel.unit()?,
                    comment: channel.comment()?,
                    data_type: PyDataType::from(&block.data_type),
                    bit_count: block.bit_count,
                };
                channels.push(info);
            }
            
            Ok(channels)
        } else {
            Err(MdfException::new_err("Group index out of bounds"))
        }
    }
    
    /// Return the names of every named channel across all groups.
    ///
    /// Channels without a name (``##TX`` link is null) are skipped.
    /// Duplicates are preserved when the same name appears in multiple groups.
    ///
    /// Returns
    /// -------
    /// list[str]
    fn get_all_channel_names(&self) -> PyResult<Vec<String>> {
        let mut names = Vec::new();
        for group in self.mdf.channel_groups() {
            for channel in group.channels() {
                if let Some(name) = channel.name()? {
                    names.push(name);
                }
            }
        }
        Ok(names)
    }
    
    /// Read a channel's samples as a contiguous numpy ``float64`` array.
    ///
    /// Searches every group and returns the first channel whose name matches
    /// ``channel_name``. This is the fastest read path for numeric channels —
    /// values are decoded directly into a numpy buffer, with any
    /// non-decodable / invalid samples set to ``NaN``. Conversions stored on
    /// the channel (linear, rational, table-lookup) are applied automatically.
    ///
    /// Parameters
    /// ----------
    /// channel_name : str
    ///     Exact channel name (case-sensitive).
    ///
    /// Returns
    /// -------
    /// Optional[numpy.ndarray]
    ///     1-D ``float64`` array of length ``record_count``, or ``None`` if no
    ///     channel with that name exists.
    fn get_channel_values<'py>(&self, py: Python<'py>, channel_name: &str) -> PyResult<Option<PyObject>> {
        for group in self.mdf.channel_groups() {
            for channel in group.channels() {
                if let Some(name) = channel.name()? {
                    if name == channel_name {
                        let values = channel.values_as_f64()?;
                        let array = PyArray1::from_vec_bound(py, values);
                        return Ok(Some(array.into()));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Read a channel from a *specific* group, by group and channel name.
    ///
    /// Use this when the same channel name appears in multiple groups (e.g.
    /// each group has its own ``Time``) and you need to disambiguate.
    ///
    /// Parameters
    /// ----------
    /// group_name : str
    ///     Exact channel-group name.
    /// channel_name : str
    ///     Exact channel name within that group.
    ///
    /// Returns
    /// -------
    /// Optional[numpy.ndarray]
    ///     1-D ``float64`` array, or ``None`` if either name is not found.
    fn get_channel_values_by_group_and_name<'py>(&self, py: Python<'py>, group_name: &str, channel_name: &str) -> PyResult<Option<PyObject>> {
        for group in self.mdf.channel_groups() {
            if let Some(gname) = group.name()? {
                if gname == group_name {
                    for channel in group.channels() {
                        if let Some(cname) = channel.name()? {
                            if cname == channel_name {
                                let values = channel.values_as_f64()?;
                                let array = PyArray1::from_vec_bound(py, values);
                                return Ok(Some(array.into()));
                            }
                        }
                    }
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Read a channel as a ``pandas.Series`` indexed by absolute timestamps.
    ///
    /// The series is indexed by the group's master channel (channel-type 2)
    /// converted to a ``DatetimeIndex`` — relative master values (in seconds)
    /// are added to the file's ``HD.abs_time`` start instant. If the channel
    /// has no master, or the master channel itself is being requested, the
    /// returned series uses the default integer index. If the file has no
    /// recorded start time, the master values are kept as a numeric index.
    ///
    /// Parameters
    /// ----------
    /// channel_name : str
    ///     Exact channel name; first match across all groups is used.
    ///
    /// Returns
    /// -------
    /// Optional[pandas.Series]
    ///     ``None`` if no channel with that name exists.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If pandas is not installed.
    fn get_channel_as_series(&self, py: Python, channel_name: &str) -> PyResult<Option<PyObject>> {
        let pd = check_pandas_available(py)?;
        let start_time_ns = self.mdf.start_time_ns();

        for group in self.mdf.channel_groups() {
            let channels = group.channels();

            for (ch_idx, channel) in channels.iter().enumerate() {
                if let Some(name) = channel.name()? {
                    if name == channel_name {
                        let values = channel.values_as_f64()?;
                        let py_values = PyArray1::from_vec_bound(py, values);

                        let index: PyObject = if let Some(master_idx) = find_master_channel(&channels) {
                            if master_idx != ch_idx {
                                let master_values = channels[master_idx].values_as_f64()?;
                                let py_master = PyArray1::from_vec_bound(py, master_values);

                                if let Some(start_ns) = start_time_ns {
                                    // Vectorized datetime conversion using pandas
                                    let to_datetime = pd.getattr(py, "to_datetime")?;
                                    let to_timedelta = pd.getattr(py, "to_timedelta")?;
                                    let start_ts = to_datetime.call(
                                        py, (start_ns,),
                                        Some([("unit", "ns")].into_py_dict(py))
                                    )?;
                                    let deltas = to_timedelta.call(
                                        py, (py_master.clone(),),
                                        Some([("unit", "s")].into_py_dict(py))
                                    )?;
                                    match deltas.call_method1(py, "__add__", (start_ts,)) {
                                        Ok(datetime_index) => datetime_index,
                                        Err(_) => py_master.into(),
                                    }
                                } else {
                                    py_master.into()
                                }
                            } else {
                                py.None()
                            }
                        } else {
                            py.None()
                        };

                        let series_class = pd.getattr(py, "Series")?;
                        let series = if index.is_none(py) {
                            series_class.call1(py, (py_values,))?
                        } else {
                            series_class.call(py, (py_values,), Some([("index", index)].into_py_dict(py)))?
                        };

                        series.setattr(py, "name", channel_name)?;
                        return Ok(Some(series));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Build a :class:`PyFileLayout` describing every block in this file.
    ///
    /// Useful for debugging or analysing the on-disk structure: returns the
    /// offset, size, type, link targets and unreferenced gaps for each
    /// MDF block (``##ID``, ``##HD``, ``##DG``, ``##CG``, ``##CN``, ``##DT``,
    /// ``##DL``, ``##TX``, ``##CC``, ``##SI``, ``##SD`` …).
    ///
    /// Returns
    /// -------
    /// PyFileLayout
    fn file_layout(&self) -> PyResult<PyFileLayout> {
        let layout = self.mdf.file_layout()?;
        Ok(PyFileLayout { inner: layout })
    }
}

/// Streaming writer for MDF 4 files.
///
/// Build an MDF file in five logical steps:
///
/// 1. ``writer = mf4_rs.PyMdfWriter("out.mf4")``
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
/// >>> w = mf4_rs.PyMdfWriter("demo.mf4")
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
#[pyclass(unsendable)]
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
    ///     Group name. **Currently ignored** — the underlying writer does
    ///     not yet emit ``acq_name`` / metadata for groups. Pass any value
    ///     (or ``None``) for forward compatibility.
    ///
    /// Returns
    /// -------
    /// str
    ///     Opaque group ID (e.g. ``"cg_0"``) — pass to subsequent
    ///     ``add_channel`` / ``write_record`` / ``finish_data_block`` calls.
    fn add_channel_group(&mut self, name: Option<String>) -> PyResult<String> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = writer.add_channel_group(None, |_cg| {
                // Could set channel group properties here
            })?;
            
            let py_id = format!("cg_{}", self.next_id);
            self.next_id += 1;
            self.channel_groups.insert(py_id.clone(), cg_id);
            
            Ok(py_id)
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
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
    /// data_type : PyDataType
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
    /// directly with the raw :class:`PyDataType`).
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
    /// type (the variant of :class:`PyDecodedValue` is *not* re-checked, so
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
/// An index records the byte ranges of each channel, fully resolves all
/// conversion blocks, and can be serialized to JSON. Once you have a
/// ``PyMdfIndex`` you can:
///
/// - Re-read channel values quickly without re-parsing the whole file.
/// - Compute exact byte ranges for **partial** / HTTP-range / S3 reads
///   (see :py:meth:`get_channel_byte_ranges`,
///   :py:meth:`get_channel_byte_ranges_for_records`).
/// - Persist with :py:meth:`save_to_file` and reload elsewhere with
///   :py:meth:`load_from_file` — the JSON is fully self-contained, the
///   original file is only required for the actual data reads.
///
/// **Limitation:** indexes do not support compressed (``##DZ``) data blocks.
///
/// Example
/// -------
/// >>> idx = mf4_rs.PyMdfIndex.from_file("recording.mf4")
/// >>> idx.save_to_file("recording.idx.json")
/// >>> # Later, possibly on another machine that has the same file:
/// >>> idx2 = mf4_rs.PyMdfIndex.load_from_file("recording.idx.json")
/// >>> values = idx2.read_channel_values_by_name_as_f64("Speed", "recording.mf4")
#[pyclass]
pub struct PyMdfIndex {
    index: MdfIndex,
}

#[pymethods]
impl PyMdfIndex {
    /// Build a fresh index by parsing an MDF file from disk.
    ///
    /// All conversions are resolved during construction so the index is
    /// fully self-contained afterwards.
    ///
    /// Parameters
    /// ----------
    /// path : str
    ///     Path to a ``.mf4`` file.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If the file cannot be parsed as MDF 4.10+.
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        let index = MdfIndex::from_file(path)?;
        Ok(PyMdfIndex { index })
    }
    
    /// Load a previously saved JSON index file.
    ///
    /// The companion to :py:meth:`save_to_file`. Reads do not require the
    /// original MDF file until you actually call ``read_channel_values_*``
    /// or ``get_channel_byte_ranges_*``.
    #[staticmethod]
    fn load_from_file(path: &str) -> PyResult<Self> {
        let index = MdfIndex::load_from_file(path)?;
        Ok(PyMdfIndex { index })
    }

    /// Serialize the index to JSON at ``path``.
    ///
    /// Output is dependency-free (no references back to the source MDF) and
    /// typically a few KB to a few MB depending on channel/conversion count.
    fn save_to_file(&self, path: &str) -> PyResult<()> {
        self.index.save_to_file(path)?;
        Ok(())
    }
    
    /// List every channel group in the index.
    ///
    /// Returns
    /// -------
    /// list[tuple[int, str, int]]
    ///     ``(group_index, group_name, channel_count)`` per group. Unnamed
    ///     groups appear with an empty string for the name.
    fn list_channel_groups(&self) -> Vec<(usize, String, usize)> {
        self.index.list_channel_groups()
            .into_iter()
            .map(|(idx, name, count)| (idx, name.to_string(), count))
            .collect()
    }
    
    /// List channels in a single group.
    ///
    /// Parameters
    /// ----------
    /// group_index : int
    ///     Index from :py:meth:`list_channel_groups`.
    ///
    /// Returns
    /// -------
    /// Optional[list[tuple[int, str, PyDataType]]]
    ///     ``(channel_index, channel_name, data_type)`` per channel, or
    ///     ``None`` if ``group_index`` is out of range.
    fn list_channels(&self, group_index: usize) -> Option<Vec<(usize, String, PyDataType)>> {
        self.index.list_channels(group_index)
            .map(|channels| {
                channels.into_iter()
                    .map(|(idx, name, data_type)| (idx, name.to_string(), PyDataType::from(data_type)))
                    .collect()
            })
    }
    
    /// Read every sample of a channel, identified by group + channel index.
    ///
    /// Conversions stored in the index are applied automatically.
    ///
    /// Parameters
    /// ----------
    /// group_index : int
    /// channel_index : int
    /// file_path : str
    ///     Path to the original MDF file (the index does not embed sample
    ///     bytes).
    ///
    /// Returns
    /// -------
    /// list[Optional[Union[float, int, str, bytes]]]
    ///     One entry per record. ``None`` indicates an invalid sample
    ///     (invalidation bit set, or undecodable). Otherwise the value is a
    ///     native Python type matching the channel's data type.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If indices are out of range, the file cannot be read, or the
    ///     file contains compressed (``##DZ``) blocks.
    fn read_channel_values(&self, py: Python, group_index: usize, channel_index: usize, file_path: &str) -> PyResult<Vec<Option<PyObject>>> {
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values(group_index, channel_index, &mut reader)?;
        Ok(values.into_iter().map(|opt_val| {
            opt_val.map(|dv| decoded_value_to_pyobject(dv, py))
        }).collect())
    }
    
    /// Read every sample of a channel by name (first match across groups).
    ///
    /// See :py:meth:`read_channel_values` for the return / error contract.
    /// If multiple groups contain the same channel name, prefer
    /// :py:meth:`read_channel_values_by_group_and_name`.
    fn read_channel_values_by_name(&self, py: Python, channel_name: &str, file_path: &str) -> PyResult<Vec<Option<PyObject>>> {
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values_by_name(channel_name, &mut reader)?;
        Ok(values.into_iter().map(|opt_val| {
            opt_val.map(|dv| decoded_value_to_pyobject(dv, py))
        }).collect())
    }
    
    /// Fast path: read a numeric channel as a list of ``float`` values.
    ///
    /// Several times faster than :py:meth:`read_channel_values` for numeric
    /// channels — opens the source file via ``mmap``, decodes directly into
    /// ``f64`` and skips ``DecodedValue`` boxing. Invalid / non-finite
    /// samples are returned as ``float('nan')`` rather than ``None``.
    ///
    /// Parameters
    /// ----------
    /// group_index : int
    /// channel_index : int
    /// file_path : str
    ///
    /// Returns
    /// -------
    /// list[float]
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If the channel is non-numeric (string / byte array), indices are
    ///     out of range, or the file cannot be mapped.
    fn read_channel_values_as_f64(&self, group_index: usize, channel_index: usize, file_path: &str) -> PyResult<Vec<f64>> {
        let file = std::fs::File::open(file_path).map_err(|e| MdfError::IOError(e))?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| MdfError::IOError(e))?;
        Ok(self.index.read_channel_values_from_slice_as_f64(group_index, channel_index, &mmap)?)
    }

    /// Fast path: read a numeric channel as ``list[float]``, looked up by name.
    ///
    /// Combines :py:meth:`find_channel_by_name` and
    /// :py:meth:`read_channel_values_as_f64`. Invalid samples are
    /// ``float('nan')``.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If no channel with that name exists, the channel is not numeric,
    ///     or the file cannot be mapped.
    fn read_channel_values_by_name_as_f64(&self, channel_name: &str, file_path: &str) -> PyResult<Vec<f64>> {
        let (group_index, channel_index) = self.index.find_channel_by_name_global(channel_name)
            .ok_or_else(|| MdfError::BlockSerializationError(
                format!("Channel '{}' not found", channel_name)
            ))?;
        let file = std::fs::File::open(file_path).map_err(|e| MdfError::IOError(e))?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| MdfError::IOError(e))?;
        Ok(self.index.read_channel_values_from_slice_as_f64(group_index, channel_index, &mmap)?)
    }

    /// Locate a channel by name across all groups.
    ///
    /// Returns
    /// -------
    /// Optional[tuple[int, int]]
    ///     ``(group_index, channel_index)`` of the first match, or ``None``.
    fn find_channel_by_name(&self, channel_name: &str) -> Option<(usize, usize)> {
        self.index.find_channel_by_name_global(channel_name)
    }

    /// Compute the byte ranges occupied by a channel across the file.
    ///
    /// Each tuple is ``(offset, length)`` and accounts for the channel's
    /// position inside the record layout *and* any data block splitting
    /// across multiple ``##DT`` fragments. Useful for issuing HTTP-range
    /// requests against a remote MDF file.
    fn get_channel_byte_ranges(&self, group_index: usize, channel_index: usize) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges(group_index, channel_index)?)
    }

    /// Like :py:meth:`get_channel_byte_ranges`, but limited to a record window.
    ///
    /// Parameters
    /// ----------
    /// group_index : int
    /// channel_index : int
    /// start_record : int
    ///     0-based first record to include.
    /// record_count : int
    ///     Number of records to cover (clamped to remaining records).
    fn get_channel_byte_ranges_for_records(&self, group_index: usize, channel_index: usize, start_record: u64, record_count: u64) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges_for_records(group_index, channel_index, start_record, record_count)?)
    }

    /// Summarize :py:meth:`get_channel_byte_ranges` without listing each range.
    ///
    /// Returns
    /// -------
    /// tuple[int, int]
    ///     ``(total_bytes, num_ranges)``.
    fn get_channel_byte_summary(&self, group_index: usize, channel_index: usize) -> PyResult<(u64, usize)> {
        Ok(self.index.get_channel_byte_summary(group_index, channel_index)?)
    }

    /// Byte ranges for a channel, looked up by name (first match).
    fn get_channel_byte_ranges_by_name(&self, channel_name: &str) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges_by_name(channel_name)?)
    }

    /// Look up a channel by name and return its position plus metadata.
    ///
    /// Returns
    /// -------
    /// Optional[tuple[int, int, PyChannelInfo]]
    ///     ``(group_index, channel_index, info)`` for the first match, or
    ///     ``None``. ``info.comment`` is always ``None`` for indexed
    ///     channels (comments are not stored in the index).
    fn get_channel_info_by_name(&self, channel_name: &str) -> Option<(usize, usize, PyChannelInfo)> {
        self.index.get_channel_info_by_name(channel_name).map(|(group_idx, channel_idx, channel)| {
            let info = PyChannelInfo {
                name: channel.name.clone(),
                unit: channel.unit.clone(),
                comment: None, // IndexedChannel doesn't store comment
                data_type: PyDataType::from(&channel.data_type),
                bit_count: channel.bit_count,
            };
            (group_idx, channel_idx, info)
        })
    }
    
    /// Find every ``(group_index, channel_index)`` whose channel name matches.
    ///
    /// Useful when the same name appears in multiple groups (e.g. ``"Time"``
    /// in each group).
    fn find_all_channels_by_name(&self, channel_name: &str) -> Vec<(usize, usize)> {
        self.index.find_all_channels_by_name(channel_name)
    }

    /// Total size, in bytes, of the source MDF file at the time the index
    /// was built.
    fn get_file_size(&self) -> u64 {
        self.index.file_size
    }

    /// True if any conversion in the index has its dependent ``##TX`` /
    /// ``##CC`` data resolved inline.
    ///
    /// Indexes built with :py:meth:`from_file` resolve all dependencies, so
    /// this normally returns ``True``.
    fn has_resolved_conversions(&self) -> bool {
        // Check if any channel has resolved conversion data
        for group in &self.index.channel_groups {
            for channel in &group.channels {
                if let Some(conversion) = &channel.conversion {
                    if conversion.resolved_texts.is_some() || conversion.resolved_conversions.is_some() {
                        return true;
                    }
                }
            }
        }
        false
    }
    
    /// Inspect the conversion attached to a channel.
    ///
    /// Returns
    /// -------
    /// Optional[dict]
    ///     ``None`` if the channel has no conversion. Otherwise a dict with
    ///     keys:
    ///
    ///     - ``conversion_type`` — debug rendering of the ``cc_type`` enum
    ///       (e.g. ``"Linear"``, ``"ValueToText"``).
    ///     - ``precision`` — decimal precision hint (``cc_precision``).
    ///     - ``flags`` — raw ``cc_flags`` bitfield.
    ///     - ``values_count`` — number of entries in ``cc_val``.
    ///     - ``values`` — raw ``cc_val`` numeric coefficients / table values.
    ///     - ``resolved_texts`` — dict[int, str] mapping ``cc_ref`` indices
    ///       to their resolved ``##TX`` text (when applicable).
    ///     - ``has_resolved_conversions`` — present when nested conversions
    ///       have been pre-resolved.
    ///     - ``formula`` — algebraic formula string (for type 3 only).
    fn get_conversion_info(&self, group_index: usize, channel_index: usize) -> PyResult<Option<HashMap<String, PyObject>>> {
        use pyo3::Python;

        let group = self.index.channel_groups.get(group_index)
            .ok_or_else(|| MdfException::new_err("Invalid group index"))?;

        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfException::new_err("Invalid channel index"))?;

        if let Some(conversion) = &channel.conversion {
            Python::with_gil(|py| {
                let mut info = HashMap::new();

                info.insert("conversion_type".to_string(), format!("{:?}", conversion.cc_type).to_object(py));
                info.insert("precision".to_string(), conversion.cc_precision.to_object(py));
                info.insert("flags".to_string(), conversion.cc_flags.to_object(py));
                info.insert("values_count".to_string(), conversion.cc_val_count.to_object(py));
                info.insert("values".to_string(), conversion.cc_val.to_object(py));

                // Include resolved data info if available
                if let Some(resolved_texts) = &conversion.resolved_texts {
                    let texts: HashMap<usize, String> = resolved_texts.clone();
                    info.insert("resolved_texts".to_string(), texts.to_object(py));
                }

                if let Some(_) = &conversion.resolved_conversions {
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

    /// Read a channel by group + channel name, disambiguating duplicates.
    ///
    /// Looks up the channel group by name first, then finds the channel
    /// within that specific group. ``None`` entries in the returned list
    /// mark invalid samples; valid samples are native Python values
    /// (``float`` / ``int`` / ``str`` / ``bytes``).
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If either ``group_name`` or ``channel_name`` is not found, or
    ///     the source file cannot be read.
    fn read_channel_values_by_group_and_name(&self, py: Python, group_name: &str, channel_name: &str, file_path: &str) -> PyResult<Vec<Option<PyObject>>> {
        // Find the group by name
        let mut group_index = None;
        for (idx, group) in self.index.channel_groups.iter().enumerate() {
            if let Some(ref gname) = group.name {
                if gname == group_name {
                    group_index = Some(idx);
                    break;
                }
            }
        }

        let group_idx = group_index.ok_or_else(||
            MdfException::new_err(format!("Channel group '{}' not found", group_name))
        )?;

        // Find the channel by name within the group
        let group = &self.index.channel_groups[group_idx];
        let mut channel_index = None;
        for (idx, channel) in group.channels.iter().enumerate() {
            if let Some(ref cname) = channel.name {
                if cname == channel_name {
                    channel_index = Some(idx);
                    break;
                }
            }
        }

        let channel_idx = channel_index.ok_or_else(||
            MdfException::new_err(format!("Channel '{}' not found in group '{}'", channel_name, group_name))
        )?;

        // Read the channel values using the found indices
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values(group_idx, channel_idx, &mut reader)?;
        Ok(values.into_iter().map(|opt_val| {
            opt_val.map(|dv| decoded_value_to_pyobject(dv, py))
        }).collect())
    }

    /// Read channel data as a pandas Series with time/master channel as DatetimeIndex.
    ///
    /// This method reads a channel's values from the index and returns them as a pandas Series
    /// with the time/master channel values converted to absolute timestamps as a DatetimeIndex.
    /// The timestamps are created by adding the relative time values to the MDF start time
    /// stored in the index. If no master channel is found, or if the queried channel IS the
    /// master channel, a default integer index is used. If the MDF file has no start time,
    /// falls back to numeric index.
    ///
    /// Requires pandas to be installed.
    ///
    /// # Arguments
    /// * `channel_name` - Name of the channel to read
    /// * `file_path` - Path to the MDF file
    ///
    /// # Returns
    /// Returns an error if the channel is not found, otherwise returns a pandas Series.
    ///
    /// # Errors
    /// Returns an error if:
    /// - pandas is not installed
    /// - the channel is not found
    /// - the master channel has a different number of values than the data channel
    fn read_channel_as_series(&self, py: Python, channel_name: &str, file_path: &str) -> PyResult<PyObject> {
        // Check if pandas is available
        let pd = check_pandas_available(py)?;

        // Get the MDF start time from the index for datetime conversion
        let start_time_ns = self.index.start_time_ns;

        // Find the channel
        let (group_idx, channel_idx) = self.find_channel_by_name(channel_name)
            .ok_or_else(|| MdfException::new_err(format!("Channel '{}' not found", channel_name)))?;

        // Read the channel values
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values(group_idx, channel_idx, &mut reader)?;
        let py_values: Vec<PyObject> = values.into_iter().map(|opt_val| {
            opt_val.map(|dv| decoded_value_to_pyobject(dv, py)).unwrap_or_else(|| py.None())
        }).collect();

        // Find the master/time channel for this group
        let group = &self.index.channel_groups[group_idx];
        let index: PyObject = if let Some(master_idx) = find_master_channel_indexed(&group.channels) {
            if master_idx != channel_idx {
                // Use the master channel values as index
                let master_values = self.index.read_channel_values(group_idx, master_idx, &mut reader)?;
                let py_master_values: Vec<PyObject> = master_values.into_iter().map(|opt_val| {
                    opt_val.map(|dv| decoded_value_to_pyobject(dv, py)).unwrap_or_else(|| py.None())
                }).collect();

                // Validate that lengths match
                if py_master_values.len() != py_values.len() {
                    return Err(MdfException::new_err(format!(
                        "Master channel length ({}) does not match data channel length ({}) for channel '{}'",
                        py_master_values.len(), py_values.len(), channel_name
                    )));
                }

                // Try to convert to DatetimeIndex if we have a start time
                if let Some(start_ns) = start_time_ns {
                    // Attempt to create DatetimeIndex from absolute timestamps
                    match create_datetime_index(py, &pd, &py_master_values, start_ns) {
                        Ok(datetime_index) => datetime_index,
                        Err(_) => {
                            // Fall back to numeric index if datetime conversion fails
                            py_master_values.to_object(py)
                        }
                    }
                } else {
                    // No start time, use numeric index
                    py_master_values.to_object(py)
                }
            } else {
                // This channel IS the master channel, use default index
                py.None()
            }
        } else {
            // No master channel found, use default index
            py.None()
        };

        // Create pandas Series
        let series_class = pd.getattr(py, "Series")?;
        let series = if index.is_none(py) {
            // No index specified, pandas will use default integer index
            series_class.call1(py, (py_values,))?
        } else {
            // Use the master channel values as index
            series_class.call(py, (py_values,), Some([("index", index)].into_py_dict(py)))?
        };

        // Set the series name to the channel name
        series.setattr(py, "name", channel_name)?;

        Ok(series)
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
#[pyclass]
#[derive(Clone)]
pub struct PyLinkInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub target: u64,
    #[pyo3(get)]
    pub target_type: Option<String>,
}

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
/// links : list[PyLinkInfo]
///     Outbound links to other blocks.
/// extra : Optional[str]
///     Block-type-specific extra information, when applicable.
#[pyclass]
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
#[pyclass]
#[derive(Clone)]
pub struct PyGapInfo {
    #[pyo3(get)]
    pub start: u64,
    #[pyo3(get)]
    pub end: u64,
    #[pyo3(get)]
    pub size: u64,
}

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
/// Build one with :py:meth:`from_file` or :py:meth:`PyMDF.file_layout`. Use
/// it to debug file structure, audit storage efficiency, or render a
/// human-readable map of an MDF.
#[pyclass]
pub struct PyFileLayout {
    pub(crate) inner: FileLayout,
}

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

    /// List of all discovered blocks, sorted by offset (``list[PyBlockInfo]``).
    #[getter]
    fn blocks(&self) -> Vec<PyBlockInfo> {
        self.inner.blocks.iter().cloned().map(Into::into).collect()
    }

    /// Byte ranges not covered by any visited block (``list[PyGapInfo]``).
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

/// Build a :class:`PyFileLayout` from an MDF file on disk.
///
/// Functional alias for :py:meth:`PyFileLayout.from_file`.
///
/// Parameters
/// ----------
/// path : str
///
/// Returns
/// -------
/// PyFileLayout
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

/// Wrap a Python ``float`` in a :class:`PyDecodedValue` of variant ``Float``.
///
/// Use this to feed values into :py:meth:`PyMdfWriter.write_record` for any
/// floating-point channel (32- or 64-bit; the value is truncated to the
/// channel's declared bit width on encode).
#[pyfunction]
fn create_float_value(value: f64) -> PyDecodedValue {
    PyDecodedValue::Float { value }
}

/// Wrap a Python ``int`` in a ``UnsignedInteger`` :class:`PyDecodedValue`.
///
/// Suitable for any unsigned integer channel; the value is truncated to the
/// channel's bit width on encode.
#[pyfunction]
fn create_uint_value(value: u64) -> PyDecodedValue {
    PyDecodedValue::UnsignedInteger { value }
}

/// Wrap a Python ``int`` in a ``SignedInteger`` :class:`PyDecodedValue`.
#[pyfunction]
fn create_int_value(value: i64) -> PyDecodedValue {
    PyDecodedValue::SignedInteger { value }
}

/// Wrap a Python ``str`` in a ``String`` :class:`PyDecodedValue`.
///
/// For string channels (``StringUtf8`` etc.). Encoding into the file is
/// done according to the channel's declared string data type.
#[pyfunction]
fn create_string_value(value: String) -> PyDecodedValue {
    PyDecodedValue::String { value }
}

/// Return the :class:`PyDataType` for little-endian unsigned integers.
///
/// Pair with :py:meth:`PyMdfWriter.add_channel` when you need an unsigned
/// integer channel of non-default width (otherwise see
/// :py:meth:`PyMdfWriter.add_int_channel`).
#[pyfunction]
fn create_data_type_uint_le() -> PyDataType {
    PyDataType { name: "UnsignedIntegerLE".to_string(), value: 0 }
}

/// Return the :class:`PyDataType` for little-endian IEEE-754 floats.
///
/// Pair with :py:meth:`PyMdfWriter.add_channel`. Defaults to 32 bits when
/// passed to ``add_channel`` — for f64 use :py:meth:`PyMdfWriter.add_float_channel`.
#[pyfunction]
fn create_data_type_float_le() -> PyDataType {
    PyDataType { name: "FloatLE".to_string(), value: 4 }
}

/// Return the :class:`PyDataType` for UTF-8 encoded string channels.
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
