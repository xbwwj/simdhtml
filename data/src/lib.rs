// Benchmark/test inputs.
//
// Common Crawl is streamed from data/commoncrawl/warc/*.warc.gz; SWDE is read
// from the bounded working set under data/swde/html.

mod cc;
mod http;
pub mod paths;
mod sample;
mod swde;

pub use paths::{
    data_dir, selectors_dir, skip, swde_html_dir, swde_source_dir, warc_dir, wpt_resources,
};
pub use sample::SamplePolicy;

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dataset {
    CommonCrawl,
    Swde,
}

impl Dataset {
    pub const ALL: [Dataset; 2] = [Self::CommonCrawl, Self::Swde];

    pub fn name(self) -> &'static str {
        match self {
            Self::CommonCrawl => "cc",
            Self::Swde => "swde",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CommonCrawl => "commoncrawl",
            Self::Swde => "swde",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "cc" | "commoncrawl" => Some(Self::CommonCrawl),
            "swde" => Some(Self::Swde),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub id: String,
    pub bucket: String,
    pub vertical: Option<String>,
    pub site: Option<String>,
    pub queries: Vec<String>,
    pub bytes: Vec<u8>,
}

impl Doc {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct QuerySet {
    pub name: String,
    pub queries: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Corpus {
    pub name: String,
    pub docs: Vec<Doc>,
    pub query_sets: Vec<QuerySet>,
}

impl Corpus {
    pub fn load(dataset: Dataset) -> Option<Self> {
        let policy = SamplePolicy::active();
        match Self::load_with(dataset, &policy) {
            Ok(corpus) if !corpus.docs.is_empty() => {
                println!(
                    "{}: {} docs, {:.1} MiB (limit {})",
                    corpus.name,
                    corpus.len(),
                    corpus.mib(),
                    policy.describe()
                );
                Some(corpus)
            }
            Ok(_) => None,
            Err(err) => {
                skip(dataset.label(), None);
                eprintln!("  {err}");
                None
            }
        }
    }

    pub fn load_with(dataset: Dataset, policy: &SamplePolicy) -> Result<Self, String> {
        match dataset {
            Dataset::CommonCrawl => cc::load(policy),
            Dataset::Swde => swde::load(policy),
        }
    }

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
        self.total_bytes() as f64 / (1024.0 * 1024.0)
    }

    pub fn buckets(&self) -> BTreeMap<String, Vec<usize>> {
        let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, doc) in self.docs.iter().enumerate() {
            out.entry(doc.bucket.clone()).or_default().push(i);
        }
        out
    }

    /// Buckets ordered by document size; datasets without a size
    /// stratification keep their name order.
    pub fn buckets_by_size(&self) -> Vec<(String, Vec<usize>)> {
        let mut buckets: Vec<(String, Vec<usize>)> = self.buckets().into_iter().collect();
        buckets.sort_by(|a, b| {
            cc::bucket_rank(&a.0)
                .cmp(&cc::bucket_rank(&b.0))
                .then_with(|| a.0.cmp(&b.0))
        });
        buckets
    }

    pub fn subset(&self, name: &str, ids: &[usize]) -> Subset<'_> {
        Subset {
            name: name.to_string(),
            docs: ids.iter().map(|&i| &self.docs[i]).collect(),
        }
    }

    pub fn workload_sets(&self) -> Vec<(String, Vec<String>)> {
        self.query_sets
            .iter()
            .map(|set| (set.name.clone(), set.queries.clone()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Subset<'a> {
    pub name: String,
    pub docs: Vec<&'a Doc>,
}

impl Subset<'_> {
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
        self.total_bytes() as f64 / (1024.0 * 1024.0)
    }
}

pub fn read_selector_sets(dir: &Path) -> Vec<QuerySet> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sel"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let queries = parse_selectors(&std::fs::read_to_string(&path).ok()?);
            let name = path.file_stem()?.to_string_lossy().into_owned();
            (!queries.is_empty()).then_some(QuerySet { name, queries })
        })
        .collect()
}

pub fn parse_selectors(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

pub fn size_bucket(bytes: usize) -> &'static str {
    match bytes {
        0..=8191 => "<8KiB",
        8192..=32767 => "8-32KiB",
        32768..=131071 => "32-128KiB",
        131072..=524287 => "128-512KiB",
        524288..=2097151 => "512KiB-2MiB",
        _ => ">2MiB",
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Quota {
    pub docs: usize,
    pub bytes: usize,
}
