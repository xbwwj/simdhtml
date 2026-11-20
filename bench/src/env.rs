//! Where the test/benchmark data lives.
//!
//! Default: walk up from this crate until a `data/` directory is found.  The
//! environment can point somewhere else:
//!
//! * `SIMDHTML_DATA_DIR` — root containing `wpt/`, `commoncrawl/`, `swde/`,
//!   `prepared/`.
//! * `SIMDHTML_WPT_DIR`, `SIMDHTML_CC_DIR`, `SIMDHTML_SWDE_DIR`,
//!   `SIMDHTML_PREPARED_DIR` — per-dataset overrides.
//! * `SIMDHTML_BENCH_MANIFEST` — explicit JSONL manifest for a corpus.
//! * `SIMDHTML_REQUIRE_TESTDATA=1` — missing data is an error, not a skip.

use std::path::{Path, PathBuf};

fn from_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

fn walk_up_for_data(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("data");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Root `data/` directory, or `None` when the checkout has no corpora.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(path) = from_env("SIMDHTML_DATA_DIR") {
        return Some(path);
    }
    if let Some(path) = walk_up_for_data(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        return Some(path);
    }
    std::env::current_dir()
        .ok()
        .and_then(|cwd| walk_up_for_data(&cwd))
}

/// WPT checkout root (`data/wpt`).
pub fn wpt_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_WPT_DIR").or_else(|| data_dir().map(|d| d.join("wpt")))
}

/// Common Crawl exported HTML directory (`data/commoncrawl/html`).
pub fn commoncrawl_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_CC_DIR").or_else(|| data_dir().map(|d| d.join("commoncrawl/html")))
}

/// Prepared SWDE documents (`data/prepared/swde`), falling back to the raw
/// extraction when the prepared tree has not been generated yet.
pub fn swde_dir() -> Option<PathBuf> {
    if let Some(path) = from_env("SIMDHTML_SWDE_DIR") {
        return Some(path);
    }
    let data = data_dir()?;
    let prepared = data.join("prepared/swde");
    if prepared.is_dir() {
        return Some(prepared);
    }
    let raw = data.join("swde/swde/sourceCode/sourceCode");
    raw.is_dir().then_some(raw)
}

/// `data/prepared`, which holds the generated manifests.
pub fn prepared_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_PREPARED_DIR").or_else(|| data_dir().map(|d| d.join("prepared")))
}

/// Manifest selected by `SIMDHTML_BENCH_MANIFEST`, otherwise
/// `data/prepared/manifests/<name>.jsonl` when it exists.
pub fn manifest(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SIMDHTML_BENCH_MANIFEST") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path = prepared_dir()?
        .join("manifests")
        .join(format!("{name}.jsonl"));
    path.is_file().then_some(path)
}

/// `data/prepared/workload`, where the fixed selector sets live.
pub fn workload_dir() -> Option<PathBuf> {
    prepared_dir().map(|d| d.join("workload"))
}

/// Whether a missing corpus should fail instead of skipping.
pub fn require_testdata() -> bool {
    std::env::var_os("SIMDHTML_REQUIRE_TESTDATA").is_some_and(|value| value != "0")
}

/// Print a skip/setup message.  Returns `true` when the caller may skip.
pub fn skip(name: &str, path: Option<&Path>) -> bool {
    if require_testdata() {
        panic!(
            "{name} data is required (SIMDHTML_REQUIRE_TESTDATA is set), but {} was not found",
            path.map_or_else(
                || "the default path".to_string(),
                |p| p.display().to_string()
            )
        );
    }
    eprintln!(
        "skip {name}: {} not found; see data/README.md (or set SIMDHTML_DATA_DIR)",
        path.map_or_else(|| "corpus".to_string(), |p| p.display().to_string())
    );
    true
}
