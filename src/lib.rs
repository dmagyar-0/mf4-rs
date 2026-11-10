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
pub mod signal;
pub mod types;
pub mod block_layout;
pub mod column;

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

// ---------------------------------------------------------------------------
// The cross-language API contract, re-exported at the crate root.
// Keep this list in step with `docs/api-contract.json` (checked by
// `tests/api_contract.rs`).
// ---------------------------------------------------------------------------

pub use api::mdf::Mdf;
pub use block_layout::FileLayout;
pub use blocks::common::DataType;
pub use column::{to_column_oriented, ColumnOptions, ColumnReport};
#[cfg(not(target_arch = "wasm32"))]
pub use cut::{cut_mdf_by_time, cut_mdf_by_utc};
pub use error::MdfError;
pub use index::{ByteRangeReader, MdfIndex, MdfReader, Source};
#[cfg(not(target_arch = "wasm32"))]
pub use merge::merge_files;
pub use parsing::decoder::DecodedValue;
pub use signal::Signal;
pub use types::{ChannelInfo, ConversionInfo, GroupInfo, ReadOptions, TimeRange};
pub use writer::{ColumnData, MdfWriter};

// Python bindings module
#[cfg(feature = "pyo3")]
pub mod python;

// WebAssembly (wasm-bindgen) bindings module
#[cfg(feature = "wasm")]
pub mod wasm;

// Re-export the Python module when building as an extension
#[cfg(feature = "pyo3")]
use pyo3::prelude::*;

/// Python bindings for the ``mf4-rs`` crate: a reader/writer for ASAM MDF 4
/// measurement files.
///
/// The API is the same in Rust, Python and TypeScript (see ``docs/API.md``).
/// Everything is addressed by **name** — channel-group and channel names.
///
/// Read — ``read`` returns a :class:`Signal` (values + timestamps), ``values``
/// a plain numpy ``float64`` array::
///
///     import mf4_rs
///     mdf = mf4_rs.Mdf.from_file("recording.mf4")
///     for g in mdf.groups:
///         print(g.name, g.record_count, g.channel_names)
///     speed = mdf.read("Speed")                    # Signal
///     speed.to_series()                            # pandas Series, datetime index
///     rpm = mdf.values("RPM", group="Engine")      # numpy.ndarray[float64]
///     part = mdf.read("Speed", start_time=1.0, end_time=2.5)
///
/// Read lazily / remotely — :class:`MdfIndex` has the same reading API, but
/// fetches only the bytes a call needs from its ``source`` (file or URL)::
///
///     idx = mf4_rs.MdfIndex.from_url("https://host/recording.mf4")  # metadata only
///     speed = idx.read("Speed")                    # range requests happen now
///     idx.save("recording.idx.json")
///     idx = mf4_rs.MdfIndex.load("recording.idx.json")
///     idx.set_source("recording.mf4")              # re-attach a source after load
///
/// Write::
///
///     w = mf4_rs.MdfWriter("out.mf4")
///     w.init_mdf_file()
///     g = w.add_channel_group("Engine")
///     t = w.add_time_channel(g, "Time")
///     y = w.add_float_channel(g, "Speed")
///     w.start_data_block(g)
///     w.write_record(g, [0.0, 123.4])
///     w.finish_data_block(g)
///     w.finalize()
///
/// File tools: :func:`to_column_oriented` (cloud-friendly MDF 4.20 layout),
/// :func:`cut_mdf_by_time`, :func:`cut_mdf_by_utc`, :func:`merge_files` and
/// the :class:`FileLayout` block inspector.
///
/// Every error is an :class:`MdfException`; a missing channel raises its
/// subclass :class:`ChannelNotFoundError`.
#[cfg(feature = "pyo3")]
#[pymodule]
fn _mf4_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python::init_mf4_rs_module(m)
}
