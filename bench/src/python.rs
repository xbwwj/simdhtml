//! Small helpers for the embedded-CPython benchmarks/tests.
//!
//! PyO3's auto-initialized interpreter does not inherit a uv virtualenv's
//! site-packages.  `cargo xtask bench` exports `PYTHONPATH`; this fallback
//! keeps a direct run working when `PYO3_PYTHON` points at the venv.

use pyo3_embed::{Python, types::PyAnyMethods};

/// Add the bench virtualenv's site-packages to `sys.path`.
pub fn add_venv_sys_path(py: Python<'_>) {
    let root = crate::env::data_dir().parent()?.join(".venv/lib");
    let candidate = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("site-packages"))
        .find(|path| path.is_dir());
    let Some(candidate) = candidate else {
        return;
    };
    let Ok(sys) = py.import("sys") else {
        return;
    };
    let Ok(path) = sys.getattr("path") else {
        return;
    };
    let _ = path.call_method1("insert", (0, candidate.to_string_lossy().to_string()));
}
