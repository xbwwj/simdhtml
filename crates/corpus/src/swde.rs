// Sampling walks the index, not the tree: 124k `stat` calls or any on-the-fly
// 7z decoding (pure-Rust LZMA is ~200x slower than `7z`) would dominate a run.

use std::collections::HashMap;
use std::path::Path;

use crate::{Corpus, Dataset, Doc, SamplePolicy, env, parse_selectors};

const VERTICALS: [&str; 8] = [
    "auto",
    "book",
    "camera",
    "job",
    "movie",
    "nbaplayer",
    "restaurant",
    "university",
];

struct Entry {
    size: usize,
    path: String,
    vertical: &'static str,
    site: String,
}

pub fn load(policy: &SamplePolicy) -> Result<Corpus, String> {
    let index_path = env::swde_index().ok_or("no data directory; run `make data-swde`")?;
    let html = env::swde_html_dir()
        .ok_or_else(|| format!("{} missing; run `make extract-data`", index_path.display()))?;
    let entries = read_index(&index_path, policy.max_doc_bytes)?;
    if entries.is_empty() {
        return Err(format!("{} lists no documents", index_path.display()));
    }
    let selected = sample(&entries, policy);
    let selectors = env::selectors_dir();
    let mut selector_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut docs = Vec::with_capacity(selected.len());

    for i in selected {
        let entry = &entries[i];
        let path = html.join(&entry.path);
        let bytes = std::fs::read(&path)
            .map_err(|err| format!("{}: {err}; re-run `make extract-data`", path.display()))?;
        docs.push(Doc {
            id: entry.path.clone(),
            bucket: entry.vertical.to_string(),
            vertical: Some(entry.vertical.to_string()),
            site: Some(entry.site.clone()),
            queries: doc_selectors(&mut selector_cache, selectors.as_deref(), &entry.site),
            bytes,
        });
    }

    Ok(Corpus {
        name: Dataset::Swde.label().to_string(),
        docs,
        query_sets: Vec::new(),
    })
}

fn read_index(path: &Path, max_doc_bytes: usize) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("{}: {err}; run `make extract-data`", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((size, rel)) = line.split_once('\t') else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            continue;
        };
        if size > max_doc_bytes {
            continue;
        }
        let Some(vertical) = VERTICALS.iter().find(|v| rel.starts_with(**v)) else {
            continue;
        };
        let site = rel
            .split('/')
            .nth(1)
            .map_or_else(|| "swde".to_string(), str::to_owned);
        out.push(Entry {
            size,
            path: rel.to_string(),
            vertical,
            site,
        });
    }
    Ok(out)
}

fn sample(entries: &[Entry], policy: &SamplePolicy) -> Vec<usize> {
    if policy.all {
        return (0..entries.len()).collect();
    }
    let quota = policy.quota(VERTICALS.len());
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); VERTICALS.len()];
    for (i, entry) in entries.iter().enumerate() {
        let Some(slot) = VERTICALS.iter().position(|v| *v == entry.vertical) else {
            continue;
        };
        groups[slot].push(i);
    }

    // Even stride within each vertical, so a sample covers the whole archive
    // instead of its first site.
    let lists: Vec<Vec<usize>> = groups
        .iter()
        .map(|group| {
            if group.is_empty() {
                return Vec::new();
            }
            let stride = group.len().div_ceil(quota.docs).max(1);
            group.iter().copied().step_by(stride).collect()
        })
        .collect();

    // Per-vertical byte budgets: a big vertical must not starve the others.
    let mut out = Vec::new();
    let mut cursor = vec![0usize; lists.len()];
    let mut taken = vec![0usize; lists.len()];
    let mut bytes = vec![0usize; lists.len()];
    loop {
        let mut advanced = false;
        for (slot, list) in lists.iter().enumerate() {
            if taken[slot] >= quota.docs {
                continue;
            }
            let Some(&index) = list.get(cursor[slot]) else {
                continue;
            };
            cursor[slot] += 1;
            advanced = true;
            let size = entries[index].size;
            if bytes[slot].saturating_add(size) > quota.bytes && taken[slot] > 0 {
                taken[slot] = quota.docs;
                continue;
            }
            bytes[slot] += size;
            taken[slot] += 1;
            out.push(index);
        }
        if !advanced {
            break;
        }
    }
    out
}

fn doc_selectors(
    cache: &mut HashMap<String, Vec<String>>,
    selectors: Option<&Path>,
    site: &str,
) -> Vec<String> {
    if let Some(queries) = cache.get(site) {
        return queries.clone();
    }
    let queries = selectors
        .map(|dir| dir.join("swde").join(format!("{site}.sel")))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_selectors(&text))
        .unwrap_or_default();
    cache.insert(site.to_string(), queries.clone());
    queries
}
