//! Minimal utilities for reading and writing ASAM MDF 4 files.
//!
//! The crate exposes a high level API under [`api`] to inspect existing
//! recordings as well as a [`writer::MdfWriter`] to generate new files.  Only a
//! fraction of the MDF 4 specification is implemented.

pub mod blocks;
pub mod error;
pub mod writer;
/// File-cutting utilities (native only; not available on `wasm32-unknown-unknown`).
#[cfg(not(target_arch = "wasm32"))]
pub mod cut;
/// File-merging utilities (native only; not available on `wasm32-unknown-unknown`).
#[cfg(not(target_arch = "wasm32"))]
pub mod merge;
pub mod index;
pub mod block_layout;

pub mod parsing {
    pub mod decoder;
    pub mod mdf_file;
    pub mod raw_channel_group;
    pub mod raw_data_group;
    pub mod raw_channel;
    pub mod source_info;
    pub(crate) mod reader_walk;
}

pub mod api {
    pub mod mdf;
    pub mod channel_group;
    pub mod channel;
}

// Python bindings module
#[cfg(feature = "pyo3")]
pub mod python;

// Re-export the Python module when building as an extension
#[cfg(feature = "pyo3")]
use pyo3::prelude::*;

/// Python bindings for the ``mf4-rs`` crate: a minimal reader/writer for
/// ASAM MDF 4 measurement files.
///
/// Quick tour
/// ----------
///
/// Read::
///
///     import mf4_rs
///     mdf = mf4_rs.PyMDF("recording.mf4")
///     for g in mdf.channel_groups():
///         print(g.name, g.record_count)
///     speed = mdf.get_channel_values("Speed")  # numpy.ndarray[float64]
///
/// Write::
///
///     w = mf4_rs.PyMdfWriter("out.mf4")
///     w.init_mdf_file()
///     cg = w.add_channel_group("group_0")
///     t  = w.add_time_channel(cg, "Time")
///     y  = w.add_float_channel(cg, "Speed")
///     w.start_data_block(cg)
///     w.write_record(cg, [
///         mf4_rs.create_float_value(0.0),
///         mf4_rs.create_float_value(123.4),
///     ])
///     w.finish_data_block(cg)
///     w.finalize()
///
/// Index (HTTP-friendly random access)::
///
///     idx = mf4_rs.PyMdfIndex.from_file("recording.mf4")
///     idx.save_to_file("recording.idx.json")
///     vals = idx.read_channel_values_by_name_as_f64("Speed", "recording.mf4")
///
/// Other utilities: :func:`merge_files`, :func:`cut_mdf_by_time`,
/// :func:`cut_mdf_by_utc`, and the :class:`PyFileLayout` block-layout
/// inspector.
///
/// All errors raised by this module are subclasses of :class:`MdfException`.
#[cfg(feature = "pyo3")]
#[pymodule]
fn _mf4_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python::init_mf4_rs_module(m)
}
