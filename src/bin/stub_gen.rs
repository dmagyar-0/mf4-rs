//! Generate Python type stubs (`.pyi`) for the `mf4_rs` Python module.
//!
//! Run with:
//!     cargo run --bin stub_gen --features pyo3
//!
//! Reads the `#[gen_stub_*]`-annotated declarations in `src/python.rs` and
//! emits a `.pyi` file inside the Python source tree configured in
//! `pyproject.toml` (`[tool.maturin] python-source`). Commit the generated
//! file together with any signature changes.
//!
//! Two things pyo3-stub-gen 0.7 cannot express are patched in afterwards:
//! trailing `Optional[...]` parameters get their `= None` default (PyO3
//! treats them as optional at runtime), and the exception classes are
//! declared.

use std::path::Path;

const STUB: &str = "python/mf4_rs/_mf4_rs.pyi";

const EXCEPTIONS: &str = r#"
class MdfException(Exception):
    r"""
    Base class of every error raised by mf4_rs.
    """
    ...

class ChannelNotFoundError(MdfException):
    r"""
    Raised by data operations (``read``, ``values``, ``time_range``,
    ``conversion``, ``byte_ranges``) when the named channel does not exist.
    Metadata lookups (``group``, ``channel``) return ``None`` instead.
    """
    ...
"#;

fn main() -> pyo3_stub_gen::Result<()> {
    let stub = mf4_rs::python::stub_info()?;
    stub.generate()?;

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(STUB);
    let text = std::fs::read_to_string(&path)?;
    let mut out: String = text.lines().map(|l| add_none_defaults(l) + "\n").collect();
    out.push_str(EXCEPTIONS);
    std::fs::write(&path, out)?;
    Ok(())
}

/// Give the trailing `typing.Optional[...]` parameters of a `def` line a
/// `= None` default.
fn add_none_defaults(line: &str) -> String {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("def ") {
        return line.to_string();
    }
    let (Some(open), Some(close)) = (line.find('('), line.rfind(") ->")) else {
        return line.to_string();
    };
    // Split the parameter list on top-level commas.
    let params = &line[open + 1..close];
    let mut parts: Vec<String> = Vec::new();
    let (mut depth, mut current) = (0i32, String::new());
    for ch in params.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    for part in parts.iter_mut().rev() {
        let is_optional = part.split_once(':').is_some_and(|(_, ty)| ty.trim().starts_with("typing.Optional["));
        if !is_optional || part.contains('=') {
            break;
        }
        part.push_str(" = None");
    }
    format!("{}({}{}", &line[..open], parts.join(","), &line[close..])
}
