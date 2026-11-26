use std::path::{Path, PathBuf};

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

pub fn data_dir() -> PathBuf {
    workspace_root().join("data")
}

pub fn selectors_dir() -> PathBuf {
    workspace_root().join("bench/selectors")
}

pub fn warc_dir() -> PathBuf {
    data_dir().join("commoncrawl/warc")
}

pub fn swde_source_dir() -> PathBuf {
    data_dir().join("swde/source/swde/sourceCode/sourceCode")
}

pub fn swde_html_dir() -> PathBuf {
    data_dir().join("swde/html")
}

pub fn wpt_resources() -> Option<PathBuf> {
    let dir = data_dir().join("wpt/html/syntax/parsing/resources");
    dir.is_dir().then_some(dir)
}

pub fn skip(name: &str, path: Option<&Path>) -> bool {
    eprintln!(
        "skip {name}: {} not found; run `cargo xtask data`",
        path.map_or_else(|| "the corpus".to_string(), |p| p.display().to_string())
    );
    true
}
