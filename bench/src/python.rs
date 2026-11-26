//! Small helpers for the embedded-CPython benchmarks/tests.
//!
//! PyO3's auto-initialized interpreter does not inherit a uv virtualenv's
//! site-packages. `cargo xtask bench` exports `PYTHONPATH`; this fallback
//! keeps a direct run working when `PYO3_PYTHON` points at the venv.

use pyo3_embed::{Python, types::PyAnyMethods};

/// Add the bench virtualenv's site-packages to `sys.path`.
pub fn add_venv_sys_path(py: Python<'_>) {
    let Some(candidate) = venv_site_packages() else {
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

fn venv_site_packages() -> Option<std::path::PathBuf> {
    let lib = crate::env::data_dir().parent()?.join(".venv/lib");
    std::fs::read_dir(lib)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("site-packages"))
        .find(|path| path.is_dir())
}
