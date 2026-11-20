#![allow(dead_code)]

//! Test-only data discovery, shared by the integration tests in this package.
//!
//! Default: walk up from the crate to `data/`.  `SIMDHTML_DATA_DIR`,
//! `SIMDHTML_WPT_DIR`, and `SIMDHTML_CC_DIR` override; missing data is a skip
//! unless `SIMDHTML_REQUIRE_TESTDATA=1`.

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

pub fn data_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_DATA_DIR")
        .or_else(|| walk_up_for_data(Path::new(env!("CARGO_MANIFEST_DIR"))))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| walk_up_for_data(&cwd))
        })
}

pub fn wpt_resources() -> Option<PathBuf> {
    let root = from_env("SIMDHTML_WPT_DIR").or_else(|| data_dir().map(|d| d.join("wpt")))?;
    let resources = root.join("html/syntax/parsing/resources");
    resources.is_dir().then_some(resources)
}

pub fn commoncrawl_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_CC_DIR").or_else(|| data_dir().map(|d| d.join("commoncrawl/html")))
}

pub fn skip(name: &str, path: Option<&Path>) -> bool {
    if std::env::var_os("SIMDHTML_REQUIRE_TESTDATA").is_some_and(|value| value != "0") {
        panic!(
            "{name} data is required, but {} was not found",
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
