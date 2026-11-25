// SWDE is read from the bounded working set written by `cargo xtask data`.
// The directory path carries the vertical and site; there is no index file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Corpus, Dataset, Doc, SamplePolicy, parse_selectors, paths};

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
    path: PathBuf,
    vertical: &'static str,
    site: String,
}

pub fn load(policy: &SamplePolicy) -> Result<Corpus, String> {
    let root = paths::swde_html_dir();
    if !root.is_dir() {
        return Err(format!(
            "{} missing; run `cargo xtask data`",
            root.display()
        ));
    }
    let entries = read_tree(&root, policy.max_doc_bytes)?;
    if entries.is_empty() {
        return Err(format!(
            "{} has no documents; run `cargo xtask data`",
            root.display()
        ));
    }
    let selected = sample(&entries, policy);
    let selectors = paths::selectors_dir();
    let mut selector_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut docs = Vec::with_capacity(selected.len());

    for i in selected {
        let entry = &entries[i];
        let bytes = std::fs::read(root.join(&entry.path))
            .map_err(|err| format!("{}: {err}", entry.path.display()))?;
        docs.push(Doc {
            id: entry.path.display().to_string(),
            bucket: entry.vertical.to_string(),
            vertical: Some(entry.vertical.to_string()),
            site: Some(entry.site.clone()),
            queries: doc_selectors(&mut selector_cache, &selectors, &entry.site),
            bytes,
        });
    }

    Ok(Corpus {
        name: Dataset::Swde.label().to_string(),
        docs,
        query_sets: Vec::new(),
    })
}

fn read_tree(root: &Path, max_doc_bytes: usize) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for vertical in VERTICALS {
        let dir = root.join(vertical);
        let Ok(sites) = std::fs::read_dir(&dir) else {
            continue;
        };
        for site in sites.flatten() {
            let site_path = site.path();
            if !site_path.is_dir() {
                continue;
            }
            let site_name = site.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(&site_path) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|ext| ext != "htm") {
                    continue;
                }
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                let size = meta.len() as usize;
                if size == 0 || size > max_doc_bytes {
                    continue;
                }
                out.push(Entry {
                    size,
                    path: path.strip_prefix(root).expect("under root").to_path_buf(),
                    vertical,
                    site: site_name.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
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
    selectors: &Path,
    site: &str,
) -> Vec<String> {
    if let Some(queries) = cache.get(site) {
        return queries.clone();
    }
    let path = selectors.join("swde").join(format!("{site}.sel"));
    let queries = std::fs::read_to_string(path)
        .map(|text| parse_selectors(&text))
        .unwrap_or_default();
    cache.insert(site.to_string(), queries.clone());
    queries
}
