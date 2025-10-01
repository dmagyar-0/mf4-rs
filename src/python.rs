//! Python bindings for mf4-rs using PyO3
//!
//! This module provides Python bindings for the main functionality of mf4-rs:
//! - Parsing MDF files
//! - Writing MDF files
//! - Creating and using indexes

use pyo3::prelude::*;
use pyo3::{create_exception, wrap_pyfunction};
use std::collections::HashMap;

use crate::api::mdf::MDF;
use crate::writer::MdfWriter;
use crate::index::{MdfIndex, FileRangeReader};
use crate::blocks::common::DataType;
use crate::parsing::decoder::DecodedValue;
use crate::error::MdfError;

// Custom exception for MDF errors
create_exception!(mf4_rs, MdfException, pyo3::exceptions::PyException);

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
#[pyclass(unsendable)]
pub struct PyMdfWriter {
    writer: Option<MdfWriter>,
    channel_groups: HashMap<String, String>, // Maps Python ID to Rust ID
    channels: HashMap<String, String>,       // Maps Python ID to Rust ID
    // Track the last channel added for each channel group (for automatic linking)
    last_channels: HashMap<String, String>,   // Maps channel group ID to last channel ID
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
            last_channels: HashMap::new(),
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
    
    /// Add a channel to a channel group with automatic linking and bit count
    fn add_channel(&mut self, 
                   group_id: &str, 
                   name: &str,
                   data_type: PyDataType) -> PyResult<String> {
        if let Some(ref mut writer) = self.writer {
            let cg_id = self.channel_groups.get(group_id)
                .ok_or_else(|| MdfException::new_err("Channel group not found"))?;
            
            // Automatic linking: link to the previous channel in this group
            let prev_channel_id = self.last_channels.get(group_id)
                .and_then(|py_id| self.channels.get(py_id))
                .cloned();
            
            let rust_data_type = DataType::from(data_type);
            let ch_id = writer.add_channel(cg_id, prev_channel_id.as_ref().map(|s| s.as_str()), |ch| {
                ch.data_type = rust_data_type.clone();
                ch.name = Some(name.to_string());
                // Automatic bit count from data type
                ch.bit_count = rust_data_type.default_bits();
            })?;
            
            let py_id = format!("ch_{}", self.next_id);
            self.next_id += 1;
            self.channels.insert(py_id.clone(), ch_id);
            
            // Update the last channel for this group
            self.last_channels.insert(group_id.to_string(), py_id.clone());
            
            Ok(py_id)
        } else {
            Err(MdfException::new_err("Writer has been finalized"))
        }
    }
    
    /// Add a time channel (float64, commonly used as master channel)
    fn add_time_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        let float_type = PyDataType { name: "FloatLE".to_string(), value: 4 };
        let ch_id = self.add_channel(group_id, name, float_type)?;
        // Automatically set as time/master channel
        self.set_time_channel(&ch_id)?;
        Ok(ch_id)
    }
    
    /// Add a float data channel
    fn add_float_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        let float_type = PyDataType { name: "FloatLE".to_string(), value: 4 };
        self.add_channel(group_id, name, float_type)
    }
    
    /// Add an integer data channel
    fn add_int_channel(&mut self, group_id: &str, name: &str) -> PyResult<String> {
        let uint_type = PyDataType { name: "UnsignedIntegerLE".to_string(), value: 0 };
        self.add_channel(group_id, name, uint_type)
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
    
    /// Get byte ranges for a specific record range of a channel
    fn get_channel_byte_ranges_for_records(&self, group_index: usize, channel_index: usize, start_record: u64, record_count: u64) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges_for_records(group_index, channel_index, start_record, record_count)?)
    }
    
    /// Get byte range summary for a channel (total bytes, number of ranges)
    fn get_channel_byte_summary(&self, group_index: usize, channel_index: usize) -> PyResult<(u64, usize)> {
        Ok(self.index.get_channel_byte_summary(group_index, channel_index)?)
    }
    
    /// Get byte ranges for a channel by name
    fn get_channel_byte_ranges_by_name(&self, channel_name: &str) -> PyResult<Vec<(u64, u64)>> {
        Ok(self.index.get_channel_byte_ranges_by_name(channel_name)?)
    }
    
    /// Get channel info by name (returns group_index, channel_index, and channel info)
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
    
    /// Find all channels with a given name across all groups
    fn find_all_channels_by_name(&self, channel_name: &str) -> Vec<(usize, usize)> {
        self.index.find_all_channels_by_name(channel_name)
    }
    
    /// Get file size from the index
    fn get_file_size(&self) -> u64 {
        self.index.file_size
    }
    
    /// Check if the index has resolved conversion data (enhanced index)
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
    
    /// Get conversion info for a specific channel
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

/// The main Python module initialization function
pub fn init_mf4_rs_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("MdfException", m.py().get_type_bound::<MdfException>())?;
    
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
