// Common Crawl is a source of real pages, not a model of the production
// workload: a crawler may spend most of its time on rendered React output,
// while the WARC only contains the server-rendered HTML.  The public sample is
// therefore deliberately stratified by size bucket: keep 200 URL-hash-selected
// documents per bucket, then even-stride down to the document/byte budget.
// The bucket labels also drive the internal size-group diagnostics.

use std::collections::HashMap;
use std::path::PathBuf;

use sha1_smol::Sha1;
use warc::{RecordType, WarcHeader, WarcReader};

use crate::{Corpus, Dataset, Doc, SamplePolicy, env, http};

const BUCKETS: [&str; 6] = [
    "<8KiB",
    "8-32KiB",
    "32-128KiB",
    "128-512KiB",
    "512KiB-2MiB",
    ">2MiB",
];

const PER_BUCKET: usize = 200;

#[derive(Clone, Copy)]
struct Candidate {
    index: usize,
    size: usize,
    hash: [u8; 20],
}

fn bucket_index(len: usize) -> usize {
    match len {
        0..=8191 => 0,
        8192..=32767 => 1,
        32768..=131071 => 2,
        131072..=524287 => 3,
        524288..=2097151 => 4,
        _ => 5,
    }
}

fn hash_url(url: &str) -> [u8; 20] {
    Sha1::from(format!("0:{url}")).digest().bytes()
}

fn for_each_payload(
    policy: &SamplePolicy,
    mut f: impl FnMut(usize, String, Vec<u8>) -> bool,
) -> Result<(), String> {
    let warc = find_warc()?;
    let reader =
        WarcReader::from_path_gzip(&warc).map_err(|err| format!("{}: {err}", warc.display()))?;
    let mut index = 0usize;
    for record in reader.iter_records() {
        let record = record.map_err(|err| format!("WARC record: {err}"))?;
        if record.warc_type() != &RecordType::Response {
            continue;
        }
        let Ok(parsed) = http::parse(record.body()) else {
            continue;
        };
        if parsed.status != "200" || !http::is_html(&parsed) {
            continue;
        }
        let Ok(payload) = http::decode_payload(&parsed) else {
            continue;
        };
        if payload.is_empty() || payload.len() > policy.max_doc_bytes {
            continue;
        }
        let url = record
            .header(WarcHeader::TargetURI)
            .map_or_else(|| index.to_string(), |value| value.into_owned());
        if !f(index, url, payload) {
            break;
        }
        index += 1;
    }
    Ok(())
}

fn select(sizes: &[usize], policy: &SamplePolicy) -> Vec<usize> {
    if policy.all || sizes.is_empty() {
        return (0..sizes.len()).collect();
    }
    let limit = policy.limit.min(sizes.len()).max(1);
    let step = sizes.len().div_ceil(limit);
    let mut selected = Vec::with_capacity(limit);
    let mut bytes = 0usize;
    for (i, &size) in sizes.iter().enumerate().step_by(step) {
        if selected.len() >= limit {
            break;
        }
        if bytes.saturating_add(size) > policy.bytes && !selected.is_empty() {
            break;
        }
        bytes += size;
        selected.push(i);
    }
    selected
}

pub fn load(policy: &SamplePolicy) -> Result<Corpus, String> {
    let mut candidates: Vec<Vec<Candidate>> = (0..BUCKETS.len()).map(|_| Vec::new()).collect();
    for_each_payload(policy, |index, url, payload| {
        let bucket = bucket_index(payload.len());
        candidates[bucket].push(Candidate {
            index,
            size: payload.len(),
            hash: hash_url(&url),
        });
        true
    })?;

    let per_bucket = if policy.all { usize::MAX } else { PER_BUCKET };
    let mut order: Vec<usize> = (0..BUCKETS.len()).collect();
    order.sort_by_key(|&bucket| BUCKETS[bucket]);
    let mut pool = Vec::new();
    for bucket in order {
        candidates[bucket].sort_by_key(|candidate| (candidate.hash, candidate.index));
        candidates[bucket].truncate(per_bucket);
        pool.extend_from_slice(&candidates[bucket]);
    }

    let sizes: Vec<usize> = pool.iter().map(|candidate| candidate.size).collect();
    let selected: Vec<Candidate> = select(&sizes, policy)
        .into_iter()
        .map(|i| pool[i])
        .collect();
    let positions: HashMap<usize, usize> = selected
        .iter()
        .enumerate()
        .map(|(position, candidate)| (candidate.index, position))
        .collect();

    let mut docs: Vec<Option<Doc>> = (0..selected.len()).map(|_| None).collect();
    if !selected.is_empty() {
        for_each_payload(policy, |index, _url, payload| {
            if let Some(&position) = positions.get(&index) {
                docs[position] = Some(Doc {
                    id: format!("{position:06}.html"),
                    bucket: BUCKETS[bucket_index(payload.len())].to_string(),
                    vertical: None,
                    site: None,
                    queries: Vec::new(),
                    bytes: payload,
                });
            }
            docs.iter().any(Option::is_none)
        })?;
    }
    let docs: Vec<Doc> = docs.into_iter().flatten().collect();

    let query_sets = env::selectors_dir()
        .map(|dir| crate::read_selector_sets(&dir.join("cc")))
        .unwrap_or_default();

    Ok(Corpus {
        name: Dataset::CommonCrawl.label().to_string(),
        docs,
        query_sets,
    })
}

fn find_warc() -> Result<PathBuf, String> {
    let Some(dir) = env::warc_dir() else {
        return Err("no data directory; run `make data-cc`".to_string());
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|err| format!("{}: {err}; run `make data-cc`", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "gz" || ext == "warc")
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .next()
        .ok_or_else(|| format!("no .warc.gz under {}; run `make data-cc`", dir.display()))
}
