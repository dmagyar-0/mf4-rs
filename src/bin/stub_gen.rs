//! Generate Python type stubs (`.pyi`) for the `mf4_rs` Python module.
//!
//! Run with:
//!     cargo run --bin stub_gen --features pyo3
//!
//! Reads the `#[gen_stub_*]`-annotated declarations in `src/python.rs` and
//! emits a `.pyi` file inside the Python source tree configured in
//! `pyproject.toml` (`[tool.maturin] python-source`). Commit the generated
//! file together with any signature changes.

fn main() -> pyo3_stub_gen::Result<()> {
    let stub = mf4_rs::python::stub_info()?;
    stub.generate()?;
    Ok(())
}
