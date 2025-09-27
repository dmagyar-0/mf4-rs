//! Python bindings for mf4-rs using PyO3
//!
//! This module provides Python bindings for the main functionality of mf4-rs:
//! - Parsing MDF files
//! - Writing MDF files
//! - Creating and using indexes

use pyo3::prelude::*;
use pyo3::{exceptions::PyException, types::PyList, wrap_pyfunction};
use std::collections::HashMap;

use crate::api::mdf::MDF;
use crate::writer::MdfWriter;
use crate::index::{MdfIndex, FileRangeReader};
use crate::blocks::common::DataType;
use crate::parsing::decoder::DecodedValue;
use crate::error::MdfError;

// Custom exception for MDF errors
create_exception!(mf4_rs, MdfException, PyException);

// Convert Rust MdfError to Python exception
impl From<MdfError> for PyErr {
    fn from(err: MdfError) -> PyErr {
        MdfException::new_err(format!("{:?}", err))
    }
}

// Python-friendly data type enum
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

// Python-friendly decoded value
#[pyclass]
#[derive(Debug, Clone)]
pub enum PyDecodedValue {
    Float { value: f64 },
    UnsignedInteger { value: u64 },
    SignedInteger { value: i64 },
    String { value: String },
    ByteArray { value: Vec<u8> },
    Unknown,
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
            PyDecodedValue::Unknown => "Unknown".to_string(),
        }
    }
    
    fn __repr__(&self) -> String {
        match self {
            PyDecodedValue::Float { value } => format!("Float({})", value),
            PyDecodedValue::UnsignedInteger { value } => format!("UnsignedInteger({})", value),
            PyDecodedValue::SignedInteger { value } => format!("SignedInteger({})", value),
            PyDecodedValue::String { value } => format!("String('{}')", value),
            PyDecodedValue::ByteArray { value } => format!("ByteArray({})", value.len()),
            PyDecodedValue::Unknown => "Unknown".to_string(),
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
            PyDecodedValue::Unknown => py.None(),
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
            DecodedValue::Unknown => PyDecodedValue::Unknown,
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
            PyDecodedValue::Unknown => DecodedValue::Unknown,
        }
    }
}

// Simplified Channel info structure to avoid lifetime issues
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

// Simplified ChannelGroup info structure
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

// Python wrapper for MDF
#[pyclass]
pub struct PyMDF {
    mdf: Box<MDF>,
}

#[pymethods]
impl PyMDF {
    /// Create a new PyMDF from a file path
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let mdf = Box::new(MDF::from_file(path)?);
        Ok(PyMDF { mdf })
    }
    
    /// Get all channel groups info
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
    
    /// Get channel info for all channels in all groups
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
    
    /// Get channels for a specific group by index
    fn get_channels_for_group(&self, group_index: usize) -> PyResult<Vec<PyChannelInfo>> {
        let groups: Vec<_> = self.mdf.channel_groups().collect();
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
    
    /// Get channel names from all groups
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
    
    /// Get channel values by name (first match)
    fn get_channel_values(&self, channel_name: &str) -> PyResult<Option<Vec<PyDecodedValue>>> {
        for group in self.mdf.channel_groups() {
            for channel in group.channels() {
                if let Some(name) = channel.name()? {
                    if name == channel_name {
                        let values = channel.values()?;
                        return Ok(Some(values.into_iter().map(PyDecodedValue::from).collect()));
                    }
                }
            }
        }
        Ok(None)
    }
}

// Python wrapper for MdfWriter
#[pyclass]
pub struct PyMdfWriter {
    writer: Option<MdfWriter>,
    channel_groups: HashMap<String, String>, // Maps Python ID to Rust ID
    channels: HashMap<String, String>,       // Maps Python ID to Rust ID
    next_id: usize,
}

#[pymethods]
impl PyMdfWriter {
    /// Create a new MdfWriter
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let writer = MdfWriter::new(path)?;
        Ok(PyMdfWriter {
            writer: Some(writer),
            channel_groups: HashMap::new(),
            channels: HashMap::new(),
            next_id: 0,
        })
    }
    
    /// Initialize the MDF file
    fn init_mdf_file(&mut self) -> PyResult<()> {
        if let Some(ref mut writer) = self.writer {
            writer.init_mdf_file()?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Add a channel group
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
    
    /// Add a channel to a channel group
    fn add_channel(&mut self, 
                   group_id: &str, 
                   name: Option<String>,
                   data_type: PyDataType,
                   bit_count: u32,
                   master_channel_id: Option<String>) -> PyResult<String> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            
            let master_id = if let Some(master_py_id) = master_channel_id {
                self.channels.get(&master_py_id).cloned()
            } else {
                None
            };
            
            let rust_data_type = DataType::from(data_type);
            let ch_id = writer.add_channel(cg_id, master_id.as_ref(), |ch| {
                ch.data_type = rust_data_type;
                ch.name = name;
                ch.bit_count = bit_count;
            })?;
            
            let py_id = format!("ch_{}", self.next_id);
            self.next_id += 1;
            self.channels.insert(py_id.clone(), ch_id);
            
            Ok(py_id)
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Set a channel as time/master channel
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
    
    /// Start data block for a channel group
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
    
    /// Write a record to a channel group
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
    
    /// Finish data block for a channel group
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
    
    /// Finalize the writer and close the file
    fn finalize(&mut self) -> PyResult<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize()?;
            Ok(())
        } else {
            Err(MdfException::new_err("Writer already finalized"))
        }
    }
}

// Python wrapper for MdfIndex
#[pyclass]
pub struct PyMdfIndex {
    index: MdfIndex,
}

#[pymethods]
impl PyMdfIndex {
    /// Create index from MDF file
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        let index = MdfIndex::from_file(path)?;
        Ok(PyMdfIndex { index })
    }
    
    /// Load index from JSON file
    #[staticmethod]
    fn load_from_file(path: &str) -> PyResult<Self> {
        let index = MdfIndex::load_from_file(path)?;
        Ok(PyMdfIndex { index })
    }
    
    /// Save index to JSON file
    fn save_to_file(&self, path: &str) -> PyResult<()> {
        self.index.save_to_file(path)?;
        Ok(())
    }
    
    /// List all channel groups
    fn list_channel_groups(&self) -> Vec<(usize, String, usize)> {
        self.index.list_channel_groups()
            .into_iter()
            .map(|(idx, name, count)| (idx, name.to_string(), count))
            .collect()
    }
    
    /// List channels in a specific group
    fn list_channels(&self, group_index: usize) -> Option<Vec<(usize, String, PyDataType)>> {
        self.index.list_channels(group_index)
            .map(|channels| {
                channels.into_iter()
                    .map(|(idx, name, data_type)| (idx, name.to_string(), PyDataType::from(data_type)))
                    .collect()
            })
    }
    
    /// Read channel values by index
    fn read_channel_values(&self, group_index: usize, channel_index: usize, file_path: &str) -> PyResult<Vec<PyDecodedValue>> {
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values(group_index, channel_index, &mut reader)?;
        Ok(values.into_iter().map(PyDecodedValue::from).collect())
    }
    
    /// Read channel values by name
    fn read_channel_values_by_name(&self, channel_name: &str, file_path: &str) -> PyResult<Vec<PyDecodedValue>> {
        let mut reader = FileRangeReader::new(file_path)?;
        let values = self.index.read_channel_values_by_name(channel_name, &mut reader)?;
        Ok(values.into_iter().map(PyDecodedValue::from).collect())
    }
    
    /// Find channel by name
    fn find_channel_by_name(&self, channel_name: &str) -> Option<(usize, usize)> {
        self.index.find_channel_by_name_global(channel_name)
    }
    
    /// Get byte ranges for a channel
    fn get_channel_byte_ranges(&self, group_index: usize, channel_index: usize) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges(group_index, channel_index)?)
    }
}

// Helper functions

/// Create a float decoded value
#[pyfunction]
fn create_float_value(value: f64) -> PyDecodedValue {
    PyDecodedValue::Float { value }
}

/// Create an unsigned integer decoded value  
#[pyfunction]
fn create_uint_value(value: u64) -> PyDecodedValue {
    PyDecodedValue::UnsignedInteger { value }
}

/// Create a signed integer decoded value
#[pyfunction]
fn create_int_value(value: i64) -> PyDecodedValue {
    PyDecodedValue::SignedInteger { value }
}

/// Create a string decoded value
#[pyfunction]
fn create_string_value(value: String) -> PyDecodedValue {
    PyDecodedValue::String { value }
}

/// Create data types
#[pyfunction]
fn create_data_type_uint_le() -> PyDataType {
    PyDataType { name: "UnsignedIntegerLE".to_string(), value: 0 }
}

#[pyfunction]
fn create_data_type_float_le() -> PyDataType {
    PyDataType { name: "FloatLE".to_string(), value: 4 }
}

#[pyfunction]
fn create_data_type_string_utf8() -> PyDataType {
    PyDataType { name: "StringUtf8".to_string(), value: 7 }
}

/// The main Python module
#[pymodule]
fn mf4_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add("MdfException", _py.get_type::<MdfException>())?;
    
    // Classes
    m.add_class::<PyMDF>()?;
    m.add_class::<PyMdfWriter>()?;
    m.add_class::<PyMdfIndex>()?;
    m.add_class::<PyChannelGroupInfo>()?;
    m.add_class::<PyChannelInfo>()?;
    m.add_class::<PyDecodedValue>()?;
    m.add_class::<PyDataType>()?;
    
    // Helper functions
    m.add_function(wrap_pyfunction!(create_float_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_uint_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_int_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_string_value, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_uint_le, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_float_le, m)?)?;
    m.add_function(wrap_pyfunction!(create_data_type_string_utf8, m)?)?;
    
    Ok(())
}