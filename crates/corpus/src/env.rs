use std::path::{Path, PathBuf};

fn from_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

fn walk_up(start: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(rel);
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn walk_up_from_here(rel: &str) -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| walk_up(&cwd, rel))
        .or_else(|| walk_up(Path::new(env!("CARGO_MANIFEST_DIR")), rel))
}

pub fn data_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_DATA_DIR").or_else(|| walk_up_from_here("data"))
}

pub fn selectors_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_SELECTORS_DIR").or_else(|| walk_up_from_here("bench/selectors"))
}

pub fn wpt_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_WPT_DIR").or_else(|| data_dir().map(|d| d.join("wpt")))
}

pub fn wpt_resources() -> Option<PathBuf> {
    let dir = wpt_dir()?.join("html/syntax/parsing/resources");
    dir.is_dir().then_some(dir)
}

pub fn warc_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_WARC_DIR").or_else(|| data_dir().map(|d| d.join("commoncrawl/warc")))
}

pub fn swde_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_SWDE_DIR").or_else(|| data_dir().map(|d| d.join("swde")))
}

#[must_use]
pub fn swde_archive_dir(root: &Path) -> PathBuf {
    root.join("swde/sourceCode/sourceCode")
}

pub fn swde_html_dir() -> Option<PathBuf> {
    from_env("SIMDHTML_SWDE_HTML_DIR").or_else(|| data_dir().map(|d| d.join("swde-html")))
}

pub fn swde_index() -> Option<PathBuf> {
    std::env::var_os("SIMDHTML_SWDE_INDEX")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let path = data_dir()?.join("swde-index.tsv");
            path.is_file().then_some(path)
        })
}

pub fn require_testdata() -> bool {
    std::env::var_os("SIMDHTML_REQUIRE_TESTDATA").is_some_and(|value| value != "0")
}

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
        "skip {name}: {} not found; run `make data`",
        path.map_or_else(|| "the corpus".to_string(), |p| p.display().to_string())
    );
    true
}
