// WARC records are decoded until every size bucket has its quota, then the
// reader is dropped, so a quick run never scans the whole crawl.

use std::path::PathBuf;

use warc::{RecordType, WarcReader};

use crate::{Corpus, Dataset, Doc, Quota, SamplePolicy, env, http};

const BUCKETS: [&str; 6] = [
    "<8KiB",
    "8-32KiB",
    "32-128KiB",
    "128-512KiB",
    "512KiB-2MiB",
    ">2MiB",
];

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

#[derive(Default)]
struct Accum {
    docs: Vec<Doc>,
    bytes: usize,
}

pub fn load(policy: &SamplePolicy) -> Result<Corpus, String> {
    let warc = find_warc()?;
    let mut quota: [Quota; BUCKETS.len()] = [policy.quota(BUCKETS.len()); BUCKETS.len()];
    // A tiny bucket (e.g. >2MiB) may never fill its document quota and would
    // force a full WARC scan; bound the scan instead.
    if !policy.all {
        for q in &mut quota {
            q.docs = q.docs.min(policy.limit);
        }
    }
    let scan_limit = if policy.all {
        usize::MAX
    } else {
        (policy.limit * 64).max(2048)
    };

    let mut accum: Vec<Accum> = (0..BUCKETS.len()).map(|_| Accum::default()).collect();
    let mut scanned = 0usize;
    let mut seen = 0usize;

    let reader =
        WarcReader::from_path_gzip(&warc).map_err(|err| format!("{}: {err}", warc.display()))?;
    for record in reader.iter_records() {
        let record = record.map_err(|err| format!("WARC record: {err}"))?;
        if record.warc_type() != &RecordType::Response {
            continue;
        }
        if scanned >= scan_limit {
            break;
        }
        scanned += 1;
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
        let slot = bucket_index(payload.len());
        let acc = &mut accum[slot];
        let q = quota[slot];
        if acc.docs.len() >= q.docs || acc.bytes >= q.bytes {
            continue;
        }
        if acc.bytes + payload.len() > q.bytes && !acc.docs.is_empty() {
            continue;
        }
        seen += 1;
        acc.bytes += payload.len();
        acc.docs.push(Doc {
            id: format!("{seen:06}.html"),
            bucket: BUCKETS[slot].to_string(),
            vertical: None,
            site: None,
            queries: Vec::new(),
            bytes: payload,
        });
        if accum
            .iter()
            .zip(quota)
            .all(|(acc, q)| acc.docs.len() >= q.docs || acc.bytes >= q.bytes)
        {
            break;
        }
    }

    // Documents stay grouped by size bucket, which is also the bench order.
    let mut docs = Vec::new();
    for acc in &mut accum {
        docs.append(&mut acc.docs);
    }

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
