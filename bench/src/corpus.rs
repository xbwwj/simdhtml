//! Real benchmark corpora.  A manifest (`data/prepared/manifests/*.jsonl`) is
//! authoritative when present; otherwise the raw dataset directory is scanned
//! with the same filtering and a deterministic even sample.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MIB: usize = 1024 * 1024;
const DEFAULT_LIMIT: usize = 500;
const DEFAULT_BYTES: usize = 64 * MIB;
const DEFAULT_MAX_DOC_BYTES: usize = 4 * MIB;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dataset {
    CommonCrawl,
    Swde,
}

impl Dataset {
    pub fn name(self) -> &'static str {
        match self {
            Dataset::CommonCrawl => "cc",
            Dataset::Swde => "swde",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dataset::CommonCrawl => "commoncrawl",
            Dataset::Swde => "swde",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub id: String,
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub bucket: String,
    pub queries: Option<PathBuf>,
    pub vertical: Option<String>,
    pub site: Option<String>,
}

#[derive(Debug, Default)]
pub struct Corpus {
    pub name: String,
    pub docs: Vec<Doc>,
}

impl Corpus {
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.docs.iter().map(|doc| doc.bytes.len() as u64).sum()
    }

    pub fn mib(&self) -> f64 {
        self.total_bytes() as f64 / MIB as f64
    }

    /// A named subset, used for size-bucket groups.
    pub fn subset(&self, name: &str, ids: &[usize]) -> Self {
        Self {
            name: name.to_string(),
            docs: ids.iter().map(|&i| self.docs[i].clone()).collect(),
        }
    }

    /// Document indices grouped by the manifest's size bucket (or by
    /// `vertical/site` for SWDE).  Ordering is stable.
    pub fn buckets(&self) -> BTreeMap<String, Vec<usize>> {
        let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, doc) in self.docs.iter().enumerate() {
            out.entry(doc.bucket.clone()).or_default().push(i);
        }
        out
    }

    /// Load a corpus: manifest first, raw directory fallback.  Returns `None`
    /// (after printing the setup hint) when the data is absent.
    pub fn load(dataset: Dataset) -> Option<Self> {
        let (dir, fallback_hint) = match dataset {
            Dataset::CommonCrawl => (crate::env::commoncrawl_dir(), "./x data-cc"),
            Dataset::Swde => (crate::env::swde_dir(), "./x data-swde"),
        };
        if let Some(manifest) = crate::env::manifest(dataset.name()) {
            match Self::from_manifest(dataset, &manifest) {
                Ok(corpus) if !corpus.is_empty() => return Some(corpus),
                Ok(_) => eprintln!(
                    "{} manifest is empty: {}",
                    dataset.label(),
                    manifest.display()
                ),
                Err(err) => eprintln!("{} manifest {}: {err}", dataset.label(), manifest.display()),
            }
        }
        let Some(dir) = dir else {
            crate::env::skip(dataset.label(), None);
            eprintln!("  prepare it with: {fallback_hint}");
            return None;
        };
        if !dir.is_dir() {
            crate::env::skip(dataset.label(), Some(&dir));
            eprintln!("  prepare it with: {fallback_hint}");
            return None;
        }
        Self::from_dir(dataset, &dir)
    }

    fn from_manifest(dataset: Dataset, manifest: &Path) -> std::io::Result<Self> {
        let root = crate::env::data_dir()
            .and_then(|data| data.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let mut docs = Vec::new();
        for line in std::fs::read_to_string(manifest)?.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let row: ManifestRow = match serde_json::from_str(line) {
                Ok(row) => row,
                Err(err) => {
                    eprintln!("{} manifest row skipped: {err}", dataset.label());
                    continue;
                }
            };
            let path = resolve(&root, &row.path);
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let bucket = row
                .bucket
                .filter(|bucket| !bucket.is_empty())
                .unwrap_or_else(|| size_bucket(bytes.len()));
            docs.push(Doc {
                id: row.id,
                path,
                bytes,
                bucket,
                queries: row.queries.map(|path| resolve(&root, &path)),
                vertical: row.vertical,
                site: row.site,
            });
        }
        docs.retain(|doc| doc.bytes.len() <= max_doc_bytes());
        Ok(Self::from_docs(dataset, docs))
    }

    fn from_dir(dataset: Dataset, dir: &Path) -> Option<Self> {
        let mut paths = Vec::new();
        collect_html(dir, &mut paths);
        paths.sort();
        let docs: Vec<Doc> = paths
            .into_iter()
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                if bytes.len() > max_doc_bytes() {
                    return None;
                }
                let id = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                let bucket = match dataset {
                    Dataset::CommonCrawl => size_bucket(bytes.len()),
                    Dataset::Swde => path
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_name())
                        .map_or_else(|| "swde".to_string(), |s| s.to_string_lossy().into_owned()),
                };
                Some(Doc {
                    id,
                    path,
                    bytes,
                    bucket,
                    queries: None,
                    vertical: None,
                    site: None,
                })
            })
            .collect();
        if docs.is_empty() {
            crate::env::skip(dataset.label(), Some(dir));
            return None;
        }
        Some(Self::from_docs(dataset, docs))
    }

    fn from_docs(dataset: Dataset, docs: Vec<Doc>) -> Self {
        let docs = sample(docs);
        Self {
            name: dataset.label().to_string(),
            docs,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManifestRow {
    #[serde(default)]
    id: String,
    path: String,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    queries: Option<String>,
    #[serde(default)]
    vertical: Option<String>,
    #[serde(default)]
    site: Option<String>,
}

fn resolve(root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// Deterministic even sample over the sorted document list, bounded by both
/// `SIMDHTML_BENCH_LIMIT` and `SIMDHTML_BENCH_BYTES`.  `SIMDHTML_BENCH_ALL=1`
/// disables both bounds.
fn sample(docs: Vec<Doc>) -> Vec<Doc> {
    if env_flag("SIMDHTML_BENCH_ALL") {
        return docs;
    }
    let limit = env_usize("SIMDHTML_BENCH_LIMIT").unwrap_or(DEFAULT_LIMIT);
    let budget = env_usize("SIMDHTML_BENCH_BYTES").unwrap_or(DEFAULT_BYTES);
    if docs.len() <= limit && docs.iter().map(|d| d.bytes.len()).sum::<usize>() <= budget {
        return docs;
    }
    let step = docs.len().div_ceil(limit.max(1));
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for doc in docs.into_iter().step_by(step) {
        if out.len() >= limit || bytes + doc.bytes.len() > budget {
            break;
        }
        bytes += doc.bytes.len();
        out.push(doc);
    }
    out
}

fn max_doc_bytes() -> usize {
    env_usize("SIMDHTML_BENCH_MAX_DOC_BYTES").unwrap_or(DEFAULT_MAX_DOC_BYTES)
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value != "0")
}

fn size_bucket(bytes: usize) -> String {
    match bytes {
        0..=8191 => "<8KiB",
        8192..=32767 => "8-32KiB",
        32768..=131071 => "32-128KiB",
        131072..=524287 => "128-512KiB",
        524288..=2097151 => "512KiB-2MiB",
        _ => ">2MiB",
    }
    .to_string()
}

fn collect_html(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_html(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "html" || ext == "htm")
        {
            out.push(path);
        }
    }
}
