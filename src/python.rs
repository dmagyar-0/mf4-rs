//! Python bindings for mf4-rs (PyO3).
//!
//! This module is a thin shell over the Rust API: every class and method here
//! is one entry of the cross-language contract (`docs/API.md`,
//! `docs/api-contract.json`). `tests/test_api_contract.py` fails if the public
//! names drift from that contract — add or rename things there first.

use pyo3::prelude::*;
use pyo3::types::{IntoPyDict, PyDict};
use pyo3::{create_exception, wrap_pyfunction};
use numpy::{PyArray1, PyReadonlyArray1};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pyfunction, gen_stub_pymethods};
use pyo3_stub_gen::define_stub_info_gatherer;
use std::collections::HashMap;

use crate::api::mdf::Mdf;
use crate::writer::{MdfWriter, ColumnData};
use crate::index::MdfIndex;
use crate::blocks::common::DataType;
use crate::parsing::decoder::DecodedValue;
use crate::error::MdfError;
use crate::block_layout::{BlockInfo, FileLayout, GapInfo, LinkInfo};
use crate::signal::Signal;
use crate::types::{ChannelInfo, ConversionInfo, GroupInfo, ReadOptions, TimeRange};

// Base exception for every mf4_rs error.
create_exception!(mf4_rs, MdfException, pyo3::exceptions::PyException);
// Raised by data operations when the named channel does not exist.
create_exception!(mf4_rs, ChannelNotFoundError, MdfException);

// Convert Rust MdfError to Python exception
impl From<MdfError> for PyErr {
    fn from(err: MdfError) -> PyErr {
        match err {
            MdfError::ChannelNotFound { .. } => ChannelNotFoundError::new_err(err.to_string()),
            other => MdfException::new_err(other.to_string()),
        }
    }
}

/// Direct conversion from DecodedValue to PyObject without intermediate allocation.
fn decoded_value_to_pyobject(dv: DecodedValue, py: Python) -> PyObject {
    match dv {
        DecodedValue::Float(v) => v.to_object(py),
        DecodedValue::UnsignedInteger(v) => v.to_object(py),
        DecodedValue::SignedInteger(v) => v.to_object(py),
        DecodedValue::String(v) => v.to_object(py),
        DecodedValue::ByteArray(v) | DecodedValue::MimeSample(v) | DecodedValue::MimeStream(v) => {
            pyo3::types::PyBytes::new_bound(py, &v).into()
        }
        DecodedValue::Unknown => py.None(),
    }
}

/// Python-style repr of an optional string (`'x'` / `None`).
fn repr_opt(value: &Option<String>) -> String {
    match value {
        Some(v) => format!("'{}'", v),
        None => "None".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Contract data types
// ---------------------------------------------------------------------------

/// Metadata of one channel.
///
/// Found on :py:attr:`GroupInfo.channels` and returned by
/// :py:meth:`Mdf.channel` / :py:meth:`MdfIndex.channel`.
///
/// Attributes
/// ----------
/// name : Optional[str]
///     Channel name.
/// unit : Optional[str]
///     Physical unit.
/// comment : Optional[str]
///     Channel comment.
/// data_type : str
///     Canonical data-type name, e.g. ``"FloatLE"``, ``"UnsignedIntegerLE"``,
///     ``"StringUtf8"`` — the same names :py:meth:`MdfWriter.add_channel` takes.
/// bit_count : int
///     Width of the stored value in bits.
/// is_master : bool
///     The channel is its group's master (time) channel.
/// is_vlsd : bool
///     Variable-length (VLSD) channel, e.g. strings.
/// has_conversion : bool
///     The channel carries a conversion (applied by ``read`` / ``values``).
#[gen_stub_pyclass]
#[pyclass(name = "ChannelInfo")]
#[derive(Clone)]
pub struct PyChannelInfo {
    #[pyo3(get)]
    pub name: Option<String>,
    #[pyo3(get)]
    pub unit: Option<String>,
    #[pyo3(get)]
    pub comment: Option<String>,
    #[pyo3(get)]
    pub data_type: String,
    #[pyo3(get)]
    pub bit_count: u32,
    #[pyo3(get)]
    pub is_master: bool,
    #[pyo3(get)]
    pub is_vlsd: bool,
    #[pyo3(get)]
    pub has_conversion: bool,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyChannelInfo {
    fn __repr__(&self) -> String {
        format!(
            "ChannelInfo(name={}, unit={}, data_type='{}', bit_count={}, is_master={})",
            repr_opt(&self.name),
            repr_opt(&self.unit),
            self.data_type,
            self.bit_count,
            if self.is_master { "True" } else { "False" }
        )
    }
}

impl From<ChannelInfo> for PyChannelInfo {
    fn from(c: ChannelInfo) -> Self {
        PyChannelInfo {
            name: c.name,
            unit: c.unit,
            comment: c.comment,
            data_type: c.data_type,
            bit_count: c.bit_count,
            is_master: c.is_master,
            is_vlsd: c.is_vlsd,
            has_conversion: c.has_conversion,
        }
    }
}

/// Metadata of one channel group.
///
/// Returned by the ``groups`` property and ``group(name)`` of
/// :class:`Mdf` and :class:`MdfIndex`.
///
/// Attributes
/// ----------
/// name : Optional[str]
///     Group (acquisition) name.
/// comment : Optional[str]
///     Group comment.
/// record_count : int
///     Number of records (samples per channel).
/// channel_names : list[str]
///     Names of the group's named channels, in file order.
/// master_channel : Optional[str]
///     Name of the group's master (time) channel — local, or the remote
///     master of an MDF 4.2 column-oriented group.
/// channels : list[ChannelInfo]
///     The group's channels, in file order.
#[gen_stub_pyclass]
#[pyclass(name = "GroupInfo")]
#[derive(Clone)]
pub struct PyGroupInfo {
    #[pyo3(get)]
    pub name: Option<String>,
    #[pyo3(get)]
    pub comment: Option<String>,
    #[pyo3(get)]
    pub record_count: u64,
    #[pyo3(get)]
    pub channel_names: Vec<String>,
    #[pyo3(get)]
    pub master_channel: Option<String>,
    #[pyo3(get)]
    pub channels: Vec<PyChannelInfo>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyGroupInfo {
    fn __repr__(&self) -> String {
        format!(
            "GroupInfo(name={}, record_count={}, channel_names={:?})",
            repr_opt(&self.name),
            self.record_count,
            self.channel_names
        )
    }
}

impl From<GroupInfo> for PyGroupInfo {
    fn from(g: GroupInfo) -> Self {
        PyGroupInfo {
            name: g.name,
            comment: g.comment,
            record_count: g.record_count,
            channel_names: g.channel_names,
            master_channel: g.master_channel,
            channels: g.channels.into_iter().map(Into::into).collect(),
        }
    }
}

/// A decoded channel: its values paired with the group's time axis.
///
/// Returned by ``read`` (and ``obj[name]``) of :class:`Mdf` and
/// :class:`MdfIndex`. Call :py:meth:`to_series` for a ``pandas.Series``.
///
/// Attributes
/// ----------
/// name : str
///     Channel name.
/// unit : Optional[str]
///     Physical unit.
/// timestamps : numpy.ndarray
///     ``float64`` master-channel values (seconds relative to the measurement
///     start for a time master). Empty when the group has no master, or when
///     this channel *is* the master (its ``values`` are the time axis).
/// values : numpy.ndarray
///     One value per record, conversions applied. ``float64`` (``NaN`` =
///     invalid sample) for numeric channels; an ``object`` array of
///     ``str`` / ``bytes`` / ``None`` for text and byte channels.
///
/// Example
/// -------
/// >>> speed = mdf.read("Speed")
/// >>> speed.values.mean(), speed.unit
/// >>> speed.to_series().plot()
#[gen_stub_pyclass]
#[pyclass(name = "Signal")]
pub struct PySignal {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    unit: Option<String>,
    #[pyo3(get)]
    timestamps: PyObject,
    #[pyo3(get)]
    values: PyObject,
    start_time_ns: Option<u64>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PySignal {
    /// Number of samples.
    fn __len__(&self, py: Python) -> PyResult<usize> {
        self.values.bind(py).len()
    }

    fn __repr__(&self, py: Python) -> PyResult<String> {
        Ok(format!(
            "Signal(name='{}', unit={}, samples={})",
            self.name,
            repr_opt(&self.unit),
            self.values.bind(py).len()?
        ))
    }

    /// The signal as a ``pandas.Series``: ``values`` indexed by ``timestamps``.
    ///
    /// The index is a ``DatetimeIndex`` (timestamps added to the file's start
    /// time) when the file records a start time, otherwise the raw master
    /// seconds. Without timestamps (no master, or the master channel itself)
    /// a default integer index is used. The series is named after the channel.
    ///
    /// Raises
    /// ------
    /// MdfException
    ///     If pandas is not installed.
    fn to_series(&self, py: Python) -> PyResult<PyObject> {
        let pd = py.import_bound("pandas").map_err(|_| {
            MdfException::new_err("pandas is not installed; install it to use Signal.to_series()")
        })?;
        let timestamps: PyReadonlyArray1<f64> = self.timestamps.bind(py).extract()?;
        let timestamps = timestamps.as_slice()?;
        let series_class = pd.getattr("Series")?;
        let series = if timestamps.is_empty() {
            series_class.call1((self.values.clone_ref(py),))?
        } else {
            // Compute absolute nanosecond timestamps in Rust and hand numpy a
            // datetime64[ns] view — far faster than pd.to_timedelta. Falls
            // back to raw float seconds if anything overflows the i64 range.
            let dt_index = self.start_time_ns.and_then(|start_ns| {
                let start_i64 = i64::try_from(start_ns).ok()?;
                let mut ns: Vec<i64> = Vec::with_capacity(timestamps.len());
                for &t in timestamps {
                    let rel = t * 1e9;
                    if !rel.is_finite() || rel.abs() >= i64::MAX as f64 {
                        return None;
                    }
                    ns.push(start_i64.checked_add(rel.round() as i64)?);
                }
                let arr = PyArray1::from_vec_bound(py, ns);
                arr.call_method1("view", ("datetime64[ns]",)).ok().map(|a| a.to_object(py))
            });
            let index = dt_index.unwrap_or_else(|| self.timestamps.clone_ref(py));
            series_class.call(
                (self.values.clone_ref(py),),
                Some(&[("index", index)].into_py_dict_bound(py)),
            )?
        };
        series.setattr("name", &self.name)?;
        Ok(series.into())
    }
}

impl PySignal {
    fn new(py: Python, signal: Signal, start_time_ns: Option<u64>) -> PyResult<Self> {
        // Fast path: purely numeric (or invalid) samples become a float64
        // numpy array (NaN for invalid) instead of a per-sample object array.
        let mut numeric: Vec<f64> = Vec::with_capacity(signal.values.len());
        let mut all_numeric = true;
        for v in &signal.values {
            match v {
                None => numeric.push(f64::NAN),
                Some(DecodedValue::Float(f)) => numeric.push(*f),
                Some(DecodedValue::UnsignedInteger(u)) => numeric.push(*u as f64),
                Some(DecodedValue::SignedInteger(i)) => numeric.push(*i as f64),
                _ => {
                    all_numeric = false;
                    break;
                }
            }
        }
        let values: PyObject = if all_numeric {
            PyArray1::from_vec_bound(py, numeric).into()
        } else {
            let list: Vec<PyObject> = signal
                .values
                .into_iter()
                .map(|o| o.map(|dv| decoded_value_to_pyobject(dv, py)).unwrap_or_else(|| py.None()))
                .collect();
            let np = py.import_bound("numpy")?;
            let arr = np.call_method1("empty", (list.len(), "object"))?;
            arr.set_item(pyo3::types::PySlice::full_bound(py), list)?;
            arr.into()
        };
        Ok(PySignal {
            name: signal.name,
            unit: signal.unit,
            timestamps: PyArray1::from_vec_bound(py, signal.timestamps).into(),
            values,
            start_time_ns,
        })
    }
}

/// A resolved conversion as a plain dict (see ``conversion``).
fn conversion_to_py(py: Python, info: &ConversionInfo) -> PyResult<PyObject> {
    let d = PyDict::new_bound(py);
    d.set_item("type", &info.conversion_type)?;
    d.set_item("values", info.values.clone())?;
    d.set_item("precision", info.precision)?;
    d.set_item("flags", info.flags)?;
    if let Some([lo, hi]) = info.physical_range {
        d.set_item("physical_range", (lo, hi))?;
    }
    if let Some(formula) = &info.formula {
        d.set_item("formula", formula)?;
    }
    if !info.texts.is_empty() {
        let texts = PyDict::new_bound(py);
        for (k, v) in &info.texts {
            texts.set_item(k, v)?;
        }
        d.set_item("texts", texts)?;
    }
    if !info.refs.is_empty() {
        let refs = PyDict::new_bound(py);
        for (k, v) in &info.refs {
            refs.set_item(k, conversion_to_py(py, v)?)?;
        }
        d.set_item("refs", refs)?;
    }
    if let Some(default) = &info.default {
        d.set_item("default", conversion_to_py(py, default)?)?;
    }
    Ok(d.into())
}

fn optional_conversion_to_py(py: Python, info: Option<ConversionInfo>) -> PyResult<PyObject> {
    match info {
        Some(info) => conversion_to_py(py, &info),
        None => Ok(py.None()),
    }
}

fn time_range_to_py(range: Option<TimeRange>) -> Option<(f64, f64)> {
    range.map(|r| (r.start, r.end))
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

// ---------------------------------------------------------------------------
// Mdf (whole-file reader)
// ---------------------------------------------------------------------------

/// Reader for a whole MDF 4 file.
///
/// Only metadata is parsed up front; samples are decoded when you call
/// :py:meth:`read` / :py:meth:`values`. Everything is addressed by **name**.
/// :class:`MdfIndex` offers the same reading API for lazy / remote access.
///
/// Example
/// -------
/// >>> import mf4_rs
/// >>> mdf = mf4_rs.Mdf.from_file("recording.mf4")
/// >>> for group in mdf.groups:
/// ...     print(group.name, group.record_count, group.channel_names)
/// >>> speed = mdf.read("Speed")                 # Signal (values + timestamps)
/// >>> speed.to_series()                         # pandas Series, datetime index
/// >>> rpm = mdf.values("RPM", group="Engine")   # numpy float64 array
/// >>> part = mdf.read("Speed", start_time=1.0, end_time=2.5)
#[gen_stub_pyclass]
#[pyclass(name = "Mdf")]
pub struct PyMDF {
    mdf: Box<Mdf>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMDF {
    /// Open an MDF 4 file from disk (memory-mapped).
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        Ok(PyMDF { mdf: Box::new(Mdf::from_file(path)?) })
    }

    /// Parse an MDF 4 file from a ``bytes`` object.
    #[staticmethod]
    fn from_bytes(data: &Bound<'_, pyo3::types::PyBytes>) -> PyResult<Self> {
        Ok(PyMDF { mdf: Box::new(Mdf::from_bytes(data.as_bytes().to_vec())?) })
    }

    /// Download an MDF 4 file in full over HTTP(S) and parse it.
    ///
    /// To read a remote file *without* downloading it, use
    /// :py:meth:`MdfIndex.from_url`.
    #[staticmethod]
    fn from_url(py: Python, url: &str) -> PyResult<Self> {
        let mdf = py.allow_threads(|| Mdf::from_url(url))?;
        Ok(PyMDF { mdf: Box::new(mdf) })
    }

    /// Metadata of every channel group, in file order.
    #[getter]
    fn groups(&self) -> PyResult<Vec<PyGroupInfo>> {
        Ok(self.mdf.groups()?.into_iter().map(Into::into).collect())
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[getter]
    fn channel_names(&self) -> PyResult<Vec<String>> {
        Ok(self.mdf.channel_names()?)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or
    /// ``None`` if the file header does not record one.
    #[getter]
    fn start_time_ns(&self) -> Option<u64> {
        self.mdf.start_time_ns()
    }

    /// Metadata of the first channel group named ``name``, or ``None``.
    fn group(&self, name: &str) -> PyResult<Option<PyGroupInfo>> {
        Ok(self.mdf.group(name)?.map(Into::into))
    }

    /// Metadata of the first channel named ``name``, or ``None``.
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    ///     Look only in the group(s) with this name.
    fn channel(&self, name: &str, group: Option<&str>) -> PyResult<Option<PyChannelInfo>> {
        Ok(self.mdf.channel(name, group)?.map(Into::into))
    }

    /// Read a channel as a :class:`Signal`: its values (conversions applied)
    /// paired with the group's time axis.
    ///
    /// Parameters
    /// ----------
    /// name : str
    ///     Channel name (case-sensitive).
    /// group : Optional[str]
    ///     Look only in the group(s) with this name. Use it when the same
    ///     channel name (e.g. ``"Time"``) appears in several groups.
    /// start_time, end_time : Optional[float]
    ///     Inclusive time window, in the master channel's unit (seconds
    ///     relative to the measurement start). Omitted bounds are open.
    /// start_record, record_count : Optional[int]
    ///     Record window ``[start_record, start_record + record_count)``.
    ///     Pass either a time window or a record window, not both.
    ///
    /// Raises
    /// ------
    /// ChannelNotFoundError
    ///     If no matching channel exists.
    fn read(
        &self,
        py: Python,
        name: &str,
        group: Option<&str>,
        start_time: Option<f64>,
        end_time: Option<f64>,
        start_record: Option<u64>,
        record_count: Option<u64>,
    ) -> PyResult<PySignal> {
        let options = ReadOptions::from_parts(group, start_time, end_time, start_record, record_count);
        // Decode without holding the GIL so other Python threads can run.
        let signal = py.allow_threads(|| self.mdf.read(name, &options))?;
        PySignal::new(py, signal, self.mdf.start_time_ns())
    }

    /// Read a numeric channel as a plain numpy ``float64`` array (no
    /// timestamps). Conversions are applied; invalid and non-numeric samples
    /// are ``NaN``. Parameters as for :py:meth:`read`.
    fn values(
        &self,
        py: Python,
        name: &str,
        group: Option<&str>,
        start_time: Option<f64>,
        end_time: Option<f64>,
        start_record: Option<u64>,
        record_count: Option<u64>,
    ) -> PyResult<PyObject> {
        let options = ReadOptions::from_parts(group, start_time, end_time, start_record, record_count);
        let values = py.allow_threads(|| self.mdf.values(name, &options))?;
        Ok(PyArray1::from_vec_bound(py, values).into())
    }

    /// ``mdf["Speed"]`` — shorthand for :py:meth:`read`.
    ///
    /// Pass a ``(name, group)`` tuple to disambiguate a channel name shared by
    /// several groups, e.g. ``mdf["Speed", "Engine"]``.
    fn __getitem__(&self, py: Python, key: &Bound<'_, PyAny>) -> PyResult<PySignal> {
        let (name, group) = parse_lookup_key(key)?;
        self.read(py, &name, group.as_deref(), None, None, None, None)
    }

    /// First and last master (time) value of the group holding ``name`` as a
    /// ``(start, end)`` tuple, or ``None`` for an empty group.
    fn time_range(&self, name: &str, group: Option<&str>) -> PyResult<Option<(f64, f64)>> {
        Ok(time_range_to_py(self.mdf.time_range(name, group)?))
    }

    /// The conversion ``read`` / ``values`` apply to channel ``name``, fully
    /// resolved, or ``None`` when the channel has none.
    ///
    /// Returns a dict with ``type`` (e.g. ``"Linear"``, ``"ValueToText"``),
    /// ``values``, ``precision``, ``flags`` and, when present,
    /// ``physical_range``, ``formula``, ``texts`` (``{ref index: text}``),
    /// ``refs`` (``{ref index: nested conversion}``) and ``default``.
    fn conversion(&self, py: Python, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        optional_conversion_to_py(py, self.mdf.conversion(name, group)?)
    }

    /// Build a :class:`FileLayout` describing every block in this file.
    fn file_layout(&self) -> PyResult<PyFileLayout> {
        Ok(PyFileLayout { inner: self.mdf.file_layout()? })
    }
}

// ---------------------------------------------------------------------------
// MdfIndex (lazy / remote reader)
// ---------------------------------------------------------------------------

/// Lightweight, serialisable index of an MDF file for lazy and remote reads.
///
/// Building an index reads only metadata. The index remembers its data
/// ``source`` (a file path or ``http(s)://`` URL) and fetches just the bytes a
/// ``read`` / ``values`` call needs — with HTTP range requests for URLs. It
/// offers the same reading API as :class:`Mdf`, and can be saved as JSON and
/// re-attached to a source later.
///
/// Example
/// -------
/// >>> idx = mf4_rs.MdfIndex.from_url("https://host/recording.mf4")
/// >>> speed = idx.read("Speed", start_time=10.0, end_time=20.0)
/// >>> idx.save("recording.idx.json")
/// >>> idx = mf4_rs.MdfIndex.load("recording.idx.json")
/// >>> idx.set_source("recording.mf4")
/// >>> raw = idx.values("Speed")
#[gen_stub_pyclass]
#[pyclass(name = "MdfIndex")]
pub struct PyMdfIndex {
    index: MdfIndex,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMdfIndex {
    /// Build an index of a local MDF file. The file becomes the ``source``.
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::from_file(path)? })
    }

    /// Build an index from a ``bytes`` object holding an MDF file. No source
    /// is attached — call :py:meth:`set_source` before reading values.
    #[staticmethod]
    fn from_bytes(data: &Bound<'_, pyo3::types::PyBytes>) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::from_bytes(data.as_bytes().to_vec())? })
    }

    /// Build an index of a remote MDF file, fetching only its metadata with
    /// HTTP range requests. The URL becomes the ``source``.
    ///
    /// Parameters
    /// ----------
    /// url : str
    /// chunk_size : Optional[int]
    ///     Metadata read-ahead chunk size in bytes (default 1 MiB).
    #[staticmethod]
    fn from_url(py: Python, url: &str, chunk_size: Option<u64>) -> PyResult<Self> {
        let chunk = chunk_size.unwrap_or(1 << 20);
        let index = py.allow_threads(|| MdfIndex::from_url_with_chunk_size(url, chunk))?;
        Ok(PyMdfIndex { index })
    }

    /// Reload an index from its JSON string (see :py:meth:`to_json`). The
    /// source is not part of the JSON — attach one with :py:meth:`set_source`.
    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::from_json(json)? })
    }

    /// Reload an index from a JSON file written by :py:meth:`save`.
    #[staticmethod]
    fn load(path: &str) -> PyResult<Self> {
        Ok(PyMdfIndex { index: MdfIndex::load(path)? })
    }

    /// Serialise the index to a JSON string.
    fn to_json(&self) -> PyResult<String> {
        Ok(self.index.to_json()?)
    }

    /// Write the index as JSON to ``path``.
    fn save(&self, path: &str) -> PyResult<()> {
        Ok(self.index.save(path)?)
    }

    /// Check the index for internal consistency (e.g. after loading JSON from
    /// an untrusted place). Raises :class:`MdfException` when it is not.
    fn validate(&self) -> PyResult<()> {
        Ok(self.index.validate()?)
    }

    /// Metadata of every channel group, in file order.
    #[getter]
    fn groups(&self) -> PyResult<Vec<PyGroupInfo>> {
        Ok(self.index.groups()?.into_iter().map(Into::into).collect())
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[getter]
    fn channel_names(&self) -> PyResult<Vec<String>> {
        Ok(self.index.channel_names()?)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or
    /// ``None`` if the file header does not record one.
    #[getter]
    fn start_time_ns(&self) -> Option<u64> {
        self.index.start_time_ns()
    }

    /// Size in bytes of the indexed MDF file.
    #[getter]
    fn file_size(&self) -> u64 {
        self.index.file_size()
    }

    /// The file path or URL ``read`` / ``values`` fetch sample bytes from, or
    /// ``None``. Change it with :py:meth:`set_source`.
    #[getter]
    fn source(&self) -> Option<String> {
        self.index.source().map(|s| s.as_str().to_string())
    }

    /// Attach (or with ``None``, detach) the data source: a file path, or an
    /// ``http(s)://`` URL read with range requests.
    fn set_source(&mut self, source: Option<&str>) {
        match source {
            Some(source) => self.index.set_source(source),
            None => self.index.clear_source(),
        }
    }

    /// Metadata of the first channel group named ``name``, or ``None``.
    fn group(&self, name: &str) -> PyResult<Option<PyGroupInfo>> {
        Ok(self.index.group(name)?.map(Into::into))
    }

    /// Metadata of the first channel named ``name``, or ``None``.
    ///
    /// Parameters
    /// ----------
    /// name : str
    /// group : Optional[str]
    ///     Look only in the group(s) with this name.
    fn channel(&self, name: &str, group: Option<&str>) -> PyResult<Option<PyChannelInfo>> {
        Ok(self.index.channel(name, group)?.map(Into::into))
    }

    /// Read a channel as a :class:`Signal`, fetching only the bytes it needs
    /// from the ``source``.
    ///
    /// A window fetches only the selected records: a time window is located
    /// by searching the master channel (a few single-record reads), not by
    /// downloading it.
    ///
    /// Parameters
    /// ----------
    /// name : str
    ///     Channel name (case-sensitive).
    /// group : Optional[str]
    ///     Look only in the group(s) with this name.
    /// start_time, end_time : Optional[float]
    ///     Inclusive time window, in the master channel's unit (seconds
    ///     relative to the measurement start). Omitted bounds are open.
    /// start_record, record_count : Optional[int]
    ///     Record window ``[start_record, start_record + record_count)``.
    ///     Pass either a time window or a record window, not both.
    ///
    /// Raises
    /// ------
    /// ChannelNotFoundError
    ///     If no matching channel exists.
    /// MdfException
    ///     If no source is attached, or the read fails.
    fn read(
        &self,
        py: Python,
        name: &str,
        group: Option<&str>,
        start_time: Option<f64>,
        end_time: Option<f64>,
        start_record: Option<u64>,
        record_count: Option<u64>,
    ) -> PyResult<PySignal> {
        let options = ReadOptions::from_parts(group, start_time, end_time, start_record, record_count);
        // Release the GIL during the (potentially blocking, e.g. HTTP) read.
        let signal = py.allow_threads(|| self.index.read(name, &options))?;
        PySignal::new(py, signal, self.index.start_time_ns())
    }

    /// Read a numeric channel as a plain numpy ``float64`` array (no
    /// timestamps). Conversions are applied; invalid and non-numeric samples
    /// are ``NaN``. Parameters as for :py:meth:`read`.
    fn values(
        &self,
        py: Python,
        name: &str,
        group: Option<&str>,
        start_time: Option<f64>,
        end_time: Option<f64>,
        start_record: Option<u64>,
        record_count: Option<u64>,
    ) -> PyResult<PyObject> {
        let options = ReadOptions::from_parts(group, start_time, end_time, start_record, record_count);
        let values = py.allow_threads(|| self.index.values(name, &options))?;
        Ok(PyArray1::from_vec_bound(py, values).into())
    }

    /// ``idx["Speed"]`` — shorthand for :py:meth:`read`.
    ///
    /// Pass a ``(name, group)`` tuple to disambiguate a channel name shared by
    /// several groups, e.g. ``idx["Speed", "Engine"]``.
    fn __getitem__(&self, py: Python, key: &Bound<'_, PyAny>) -> PyResult<PySignal> {
        let (name, group) = parse_lookup_key(key)?;
        self.read(py, &name, group.as_deref(), None, None, None, None)
    }

    /// First and last master (time) value of the group holding ``name`` as a
    /// ``(start, end)`` tuple, or ``None`` for an empty group. Reads just two
    /// records from the ``source``.
    fn time_range(&self, py: Python, name: &str, group: Option<&str>) -> PyResult<Option<(f64, f64)>> {
        let range = py.allow_threads(|| self.index.time_range(name, group))?;
        Ok(time_range_to_py(range))
    }

    /// The conversion ``read`` / ``values`` apply to channel ``name``, fully
    /// resolved, or ``None`` when the channel has none. See
    /// :py:meth:`Mdf.conversion` for the returned dict.
    ///
    /// Indexes built from a URL fetch the conversion from the ``source`` on
    /// demand.
    fn conversion(&self, py: Python, name: &str, group: Option<&str>) -> PyResult<PyObject> {
        let info = py.allow_threads(|| self.index.conversion(name, group))?;
        optional_conversion_to_py(py, info)
    }

    /// ``(offset, length)`` byte ranges of the file holding a channel's
    /// samples — for issuing range requests yourself.
    ///
    /// Each range is one span per data block and includes the bytes of the
    /// group's other channels lying between this channel's samples. Refused
    /// for variable-length (VLSD) channels: use ``read`` / ``values``.
    fn byte_ranges(&self, name: &str, group: Option<&str>) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.byte_ranges(name, group)?)
    }

    /// :py:meth:`byte_ranges` for the record window
    /// ``[start_record, start_record + record_count)``.
    fn byte_ranges_for_records(
        &self,
        name: &str,
        start_record: u64,
        record_count: u64,
        group: Option<&str>,
    ) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.byte_ranges_for_records(name, start_record, record_count, group)?)
    }
}

// ---------------------------------------------------------------------------
// MdfWriter
// ---------------------------------------------------------------------------

/// Streaming writer for MDF 4 files.
///
/// Call order: :py:meth:`init_mdf_file` → :py:meth:`add_channel_group` →
/// ``add_*_channel`` (time channel first) → :py:meth:`start_data_block` →
/// :py:meth:`write_record` / :py:meth:`write_columns` →
/// :py:meth:`finish_data_block` → :py:meth:`finalize`.
///
/// Example
/// -------
/// >>> w = mf4_rs.MdfWriter("out.mf4")
/// >>> w.init_mdf_file()
/// >>> g = w.add_channel_group("Engine")
/// >>> t = w.add_time_channel(g, "Time")
/// >>> speed = w.add_float_channel(g, "Speed")
/// >>> w.add_linear_conversion(speed, 0.0, 3.6)       # m/s -> km/h
/// >>> w.start_data_block(g)
/// >>> w.write_record(g, [0.0, 12.5])
/// >>> w.write_columns(g, [numpy.arange(1.0, 4.0), numpy.ones(3)])
/// >>> w.finish_data_block(g)
/// >>> w.finalize()
#[gen_stub_pyclass]
#[pyclass(unsendable, name = "MdfWriter")]
pub struct PyMdfWriter {
    writer: Option<MdfWriter>,
    /// Channel data types per open group, cached for `write_record`.
    record_types: HashMap<String, Vec<DataType>>,
}

impl PyMdfWriter {
    fn writer(&mut self) -> PyResult<&mut MdfWriter> {
        self.writer
            .as_mut()
            .ok_or_else(|| MdfException::new_err("Writer has been finalized"))
    }
}

/// Coerce one Python value to the `DecodedValue` its channel encodes.
fn record_value(value: &Bound<'_, PyAny>, data_type: &DataType, index: usize) -> PyResult<DecodedValue> {
    let wrong = |expected: &str| {
        MdfException::new_err(format!(
            "record value at index {index} must be {expected} for a {} channel",
            data_type.name()
        ))
    };
    Ok(match data_type {
        DataType::FloatLE | DataType::FloatBE => {
            DecodedValue::Float(value.extract::<f64>().map_err(|_| wrong("a number"))?)
        }
        DataType::UnsignedIntegerLE | DataType::UnsignedIntegerBE => {
            DecodedValue::UnsignedInteger(value.extract::<u64>().map_err(|_| wrong("a non-negative int"))?)
        }
        DataType::SignedIntegerLE | DataType::SignedIntegerBE => {
            DecodedValue::SignedInteger(value.extract::<i64>().map_err(|_| wrong("an int"))?)
        }
        dt if dt.is_string() => {
            DecodedValue::String(value.extract::<String>().map_err(|_| wrong("a str"))?)
        }
        _ => DecodedValue::ByteArray(value.extract::<&[u8]>().map_err(|_| wrong("bytes"))?.to_vec()),
    })
}

#[gen_stub_pymethods]
#[pymethods]
impl PyMdfWriter {
    /// Create a writer for a new file at ``path`` (truncating an existing one).
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        Ok(PyMdfWriter { writer: Some(MdfWriter::new(path)?), record_types: HashMap::new() })
    }

    /// Write the identification and header blocks. Call once, first. The
    /// header is stamped with the current time (see :py:meth:`set_start_time`).
    fn init_mdf_file(&mut self) -> PyResult<()> {
        self.writer()?.init_mdf_file()?;
        Ok(())
    }

    /// Set the measurement start time (nanoseconds since the Unix epoch, UTC).
    fn set_start_time(&mut self, abs_time_ns: u64) -> PyResult<()> {
        Ok(self.writer()?.set_start_time(abs_time_ns)?)
    }

    /// Add a channel group and return its id.
    fn add_channel_group(&mut self, name: Option<&str>) -> PyResult<String> {
        Ok(self.writer()?.add_channel_group(name)?)
    }

    /// Set a channel group's comment.
    fn set_channel_group_comment(&mut self, group_id: &str, comment: &str) -> PyResult<()> {
        Ok(self.writer()?.set_channel_group_comment(group_id, comment)?)
    }

    /// Add a channel of the given data type (default width) and return its id.
    ///
    /// Parameters
    /// ----------
    /// group_id : str
    /// name : str
    /// data_type : str
    ///     A canonical data-type name as reported by
    ///     :py:attr:`ChannelInfo.data_type`: ``"FloatLE"``,
    ///     ``"UnsignedIntegerLE"``, ``"SignedIntegerLE"``, ``"StringUtf8"``,
    ///     ``"ByteArray"``, … Text types are written as variable-length channels.
    fn add_channel(&mut self, group_id: &str, name: &str, data_type: &str) -> PyResult<String> {
        let data_type = DataType::from_name(data_type)?;
        Ok(self.writer()?.add_channel(group_id, name, data_type)?)
    }

    /// Add a 64-bit float channel and mark it as the group's master (time)
    /// channel. Add it first, before the group's data channels.
    fn add_time_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_time_channel(group_id, name)?)
    }

    /// Add a 64-bit float channel.
    fn add_float_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_float_channel(group_id, name)?)
    }

    /// Add a 32-bit float channel.
    fn add_float32_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_float32_channel(group_id, name)?)
    }

    /// Add a 64-bit **unsigned** integer channel.
    fn add_int_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_int_channel(group_id, name)?)
    }

    /// Add a 64-bit **signed** integer channel.
    fn add_sint_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_sint_channel(group_id, name)?)
    }

    /// Add a variable-length UTF-8 string channel.
    fn add_string_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        Ok(self.writer()?.add_string_channel(group_id, name)?)
    }

    /// Mark an existing channel as its group's master (time) channel.
    fn set_time_channel(&mut self, channel_id: &str) -> PyResult<()> {
        Ok(self.writer()?.set_time_channel(channel_id)?)
    }

    /// Attach a linear conversion (``physical = offset + factor * raw``) to a
    /// channel. Call before :py:meth:`start_data_block`.
    fn add_linear_conversion(&mut self, channel_id: &str, offset: f64, factor: f64) -> PyResult<()> {
        Ok(self.writer()?.add_linear_conversion(channel_id, offset, factor)?)
    }

    /// Attach a value-to-text conversion to a channel.
    ///
    /// Parameters
    /// ----------
    /// channel_id : str
    /// mapping : Sequence[tuple[int, str]]
    ///     ``(raw value, text)`` pairs.
    /// default_text : str
    ///     Text for any raw value not in ``mapping``.
    fn add_value_to_text_conversion(
        &mut self,
        channel_id: &str,
        mapping: Vec<(i64, String)>,
        default_text: &str,
    ) -> PyResult<()> {
        let pairs: Vec<(i64, &str)> = mapping.iter().map(|(v, t)| (*v, t.as_str())).collect();
        Ok(self.writer()?.add_value_to_text_conversion(channel_id, &pairs, default_text)?)
    }

    /// Open the data block of a channel group. Call after all of the group's
    /// channels and conversions are added.
    fn start_data_block(&mut self, group_id: &str) -> PyResult<()> {
        let types = self.writer()?.channel_data_types(group_id)?;
        self.writer()?.start_data_block(group_id)?;
        self.record_types.insert(group_id.to_string(), types);
        Ok(())
    }

    /// Append one record: one plain value per channel, in the order the
    /// channels were added (``float`` / ``int`` / ``str`` / ``bytes``).
    fn write_record(&mut self, group_id: &str, values: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        let types = self
            .record_types
            .get(group_id)
            .ok_or_else(|| MdfException::new_err(format!("no open data block for group '{group_id}'")))?;
        if values.len() != types.len() {
            return Err(MdfException::new_err(format!(
                "expected {} values for this group, got {}",
                types.len(),
                values.len()
            )));
        }
        let record = values
            .iter()
            .zip(types)
            .enumerate()
            .map(|(i, (value, data_type))| record_value(value, data_type, i))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(self.writer()?.write_record(group_id, &record)?)
    }

    /// Append many records at once from one numpy array per channel (the fast
    /// path). Arrays must be 1-D, contiguous, equally long and of dtype
    /// ``float64``, ``float32``, ``uint64`` or ``int64`` matching the channel.
    fn write_columns(&mut self, group_id: &str, columns: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        // Keep the extracted arrays alive for the duration of the call.
        enum OwnedArray<'py> {
            F64(PyReadonlyArray1<'py, f64>),
            F32(PyReadonlyArray1<'py, f32>),
            U64(PyReadonlyArray1<'py, u64>),
            I64(PyReadonlyArray1<'py, i64>),
        }
        let mut owned = Vec::with_capacity(columns.len());
        for (i, col) in columns.iter().enumerate() {
            owned.push(if let Ok(a) = col.extract::<PyReadonlyArray1<f64>>() {
                OwnedArray::F64(a)
            } else if let Ok(a) = col.extract::<PyReadonlyArray1<f32>>() {
                OwnedArray::F32(a)
            } else if let Ok(a) = col.extract::<PyReadonlyArray1<u64>>() {
                OwnedArray::U64(a)
            } else if let Ok(a) = col.extract::<PyReadonlyArray1<i64>>() {
                OwnedArray::I64(a)
            } else {
                return Err(MdfException::new_err(format!(
                    "column {i} must be a 1-D numpy array of dtype float64, float32, uint64 or int64"
                )));
            });
        }
        let contiguous = |e: numpy::NotContiguousError| MdfException::new_err(format!("Array not contiguous: {e}"));
        let column_data = owned
            .iter()
            .map(|arr| match arr {
                OwnedArray::F64(a) => a.as_slice().map(ColumnData::F64).map_err(contiguous),
                OwnedArray::F32(a) => a.as_slice().map(ColumnData::F32).map_err(contiguous),
                OwnedArray::U64(a) => a.as_slice().map(ColumnData::U64).map_err(contiguous),
                OwnedArray::I64(a) => a.as_slice().map(ColumnData::I64).map_err(contiguous),
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(self.writer()?.write_columns(group_id, &column_data)?)
    }

    /// Close a channel group's data block. Call once per group after all
    /// records are written.
    fn finish_data_block(&mut self, group_id: &str) -> PyResult<()> {
        self.record_types.remove(group_id);
        Ok(self.writer()?.finish_data_block(group_id)?)
    }

    /// Flush and close the file. The writer cannot be used afterwards.
    fn finalize(&mut self) -> PyResult<()> {
        match self.writer.take() {
            Some(writer) => Ok(writer.finalize()?),
            None => Err(MdfException::new_err("Writer already finalized")),
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

/// Cut an MDF file by time, copying only records whose master channel value
/// falls within the inclusive `[start_time, end_time]` window.
///
/// The output preserves fixed-length numeric, string, and byte-array
/// channels, per-record invalidation bytes, and VLSD ("signal-based")
/// channels (a fresh ##SD chain is written for each kept VLSD channel).
/// Per-channel conversion, source, unit and comment blocks are cloned
/// recursively into the output, so cut channels keep their physical
/// scaling and metadata.
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

/// Summary of a :func:`to_column_oriented` conversion.
///
/// Attributes
/// ----------
/// source_groups : int
///     Channel groups found in the source file.
/// time_groups : int
///     Time (master) channel groups written. Each is shared by every signal of
///     its source group(s).
/// signal_groups : int
///     Signal channel groups written (one per signal).
/// shared_time_groups : int
///     Source groups whose time axis was byte-identical to an earlier group's
///     and therefore reuse that group's time channel.
/// data_blocks : int
///     Data blocks (``##DV`` / ``##DT`` / ``##SD``) written.
/// metadata_size : int
///     Byte offset where sample data starts. Everything before it is metadata,
///     so fetching this prefix is enough to build an index of the file.
/// file_size : int
///     Size of the output file in bytes.
/// skipped_channels : list[str]
///     Channels that could not be represented as a column, as
///     ``"group/channel: reason"``.
#[gen_stub_pyclass]
#[pyclass(name = "ColumnReport")]
#[derive(Clone)]
pub struct PyColumnReport {
    #[pyo3(get)]
    pub source_groups: usize,
    #[pyo3(get)]
    pub time_groups: usize,
    #[pyo3(get)]
    pub signal_groups: usize,
    #[pyo3(get)]
    pub shared_time_groups: usize,
    #[pyo3(get)]
    pub data_blocks: usize,
    #[pyo3(get)]
    pub metadata_size: u64,
    #[pyo3(get)]
    pub file_size: u64,
    #[pyo3(get)]
    pub skipped_channels: Vec<String>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyColumnReport {
    fn __repr__(&self) -> String {
        format!(
            "ColumnReport(signal_groups={}, time_groups={}, shared_time_groups={}, \
             data_blocks={}, metadata_size={}, file_size={}, skipped_channels={})",
            self.signal_groups,
            self.time_groups,
            self.shared_time_groups,
            self.data_blocks,
            self.metadata_size,
            self.file_size,
            self.skipped_channels.len()
        )
    }
}

impl From<crate::column::ColumnReport> for PyColumnReport {
    fn from(r: crate::column::ColumnReport) -> Self {
        Self {
            source_groups: r.source_groups,
            time_groups: r.time_groups,
            signal_groups: r.signal_groups,
            shared_time_groups: r.shared_time_groups,
            data_blocks: r.data_blocks,
            metadata_size: r.metadata_size,
            file_size: r.file_size,
            skipped_channels: r.skipped_channels,
        }
    }
}

/// Rewrite an MDF 4 file as a column-oriented MDF 4.20 file optimised for
/// reading single signals from cloud storage.
///
/// Each signal gets its own channel group whose data blocks are written back
/// to back, so one HTTP range request fetches the whole signal. Timestamps are
/// stored once per source group and shared by all its signals through the Mdf
/// 4.2 *remote master* link (and, with ``share_identical_time``, shared across
/// source groups that have identical time axes). All metadata blocks sit at
/// the front of the file, so building an :class:`MdfIndex` over HTTP needs a
/// single prefix fetch. Raw bytes are copied, never decoded: conversions,
/// units, comments, sources, invalidation bits and VLSD channels carry over.
///
/// The source is streamed once and memory stays bounded by ``memory_budget``
/// regardless of file size. The GIL is released during the conversion.
///
/// Parameters
/// ----------
/// input_path : str
///     Path to the source MF4 file (sorted or unsorted, uncompressed).
/// output_path : str
///     Destination path (overwritten); must differ from ``input_path``.
/// block_size : int, optional
///     Target payload size of each data block. Larger signals are split into
///     several equally sized blocks (whole records each), listed by a ``##DL``.
///     Default 4 MiB.
/// memory_budget : int, optional
///     Upper bound for the per-channel write buffers. Default 64 MiB.
/// share_identical_time : bool, optional
///     Default ``True``. Merge byte-identical time axes of different source groups into one
///     time channel group.
///
/// Returns
/// -------
/// ColumnReport
///
/// Raises
/// ------
/// MdfException
///     On I/O errors or unsupported input (e.g. compressed ``##DZ`` blocks).
///
/// Example
/// -------
/// >>> report = mf4_rs.to_column_oriented("recording.mf4", "recording.col.mf4")
/// >>> idx = mf4_rs.MdfIndex.from_url("https://host/recording.col.mf4")
/// >>> speed = idx.read("Speed")   # two range requests: values + time
#[gen_stub_pyfunction]
#[pyfunction]
fn to_column_oriented(
    py: Python<'_>,
    input_path: &str,
    output_path: &str,
    block_size: Option<u64>,
    memory_budget: Option<u64>,
    share_identical_time: Option<bool>,
) -> PyResult<PyColumnReport> {
    let defaults = crate::column::ColumnOptions::default();
    let options = crate::column::ColumnOptions {
        block_size: block_size.unwrap_or(defaults.block_size),
        memory_budget: memory_budget.unwrap_or(defaults.memory_budget),
        share_identical_time: share_identical_time.unwrap_or(defaults.share_identical_time),
        ..defaults
    };
    let report = py.allow_threads(|| {
        crate::column::to_column_oriented(input_path, output_path, &options)
    })?;
    Ok(report.into())
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
    crate::cut::cut_mdf_by_utc(input_path, output_path, start_ns, end_ns)?;
    Ok(())
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
        "Base class of every error raised by mf4_rs: malformed or unsupported \
         MDF files, I/O failures, invalid arguments, a missing data source, a \
         finalized writer, and so on.\n\n\
         >>> try:\n\
         ...     mdf = mf4_rs.Mdf.from_file(\"missing.mf4\")\n\
         ... except mf4_rs.MdfException as e:\n\
         ...     print(\"could not open:\", e)",
    )?;
    m.add("MdfException", mdf_exception)?;
    let not_found = m.py().get_type_bound::<ChannelNotFoundError>();
    not_found.setattr(
        "__doc__",
        "Raised by data operations (read, values, time_range, conversion, \
         byte_ranges) when the named channel does not exist. Metadata lookups \
         (group, channel) return None instead. Subclass of MdfException.",
    )?;
    m.add("ChannelNotFoundError", not_found)?;

    // Classes
    m.add_class::<PyMDF>()?;
    m.add_class::<PyMdfIndex>()?;
    m.add_class::<PyMdfWriter>()?;
    m.add_class::<PySignal>()?;
    m.add_class::<PyGroupInfo>()?;
    m.add_class::<PyChannelInfo>()?;
    m.add_class::<PyFileLayout>()?;
    m.add_class::<PyBlockInfo>()?;
    m.add_class::<PyLinkInfo>()?;
    m.add_class::<PyGapInfo>()?;
    m.add_class::<PyColumnReport>()?;

    // Functions
    m.add_function(wrap_pyfunction!(to_column_oriented, m)?)?;
    m.add_function(wrap_pyfunction!(cut_mdf_by_time, m)?)?;
    m.add_function(wrap_pyfunction!(cut_mdf_by_utc, m)?)?;
    m.add_function(wrap_pyfunction!(merge_files, m)?)?;

    Ok(())
}

// Generates `pub fn stub_info() -> pyo3_stub_gen::Result<StubInfo>`, used by
// `src/bin/stub_gen.rs` to walk the gathered `#[gen_stub_*]` declarations and
// emit the .pyi file. No runtime cost — the gathering only runs when the
// stub-gen binary is invoked.
define_stub_info_gatherer!(stub_info);
