//! Download and derive the corpus inputs.
//!
//! Fetching lives here because this crate owns the layout of `data/`;
//! `cargo xtask data` decides when to run each step.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::SamplePolicy;
use crate::paths;

const WARC_NAME: &str = "CC-MAIN-20261005105623-20261005135623-00993.warc.gz";
const WARC_SIZE: u64 = 958_726_164;
const WARC_URL: &str = "https://data.commoncrawl.org/crawl-data/CC-MAIN-2026-43/segments/1791170053823.21/warc/CC-MAIN-20261005105623-20261005135623-00993.warc.gz";

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

const SWDE_TMP: &str = "swde/.tmp";
const SWDE_LIST: &str = "swde/.tmp-list.txt";

struct Entry {
    archive: PathBuf,
    path: String,
    size: usize,
    vertical: &'static str,
}

/// Initializes the data submodules (SWDE sources and WPT).
pub fn submodules() -> Result<(), String> {
    exec(
        Command::new("git")
            .args([
                "submodule",
                "update",
                "--init",
                "--depth",
                "1",
                "data/swde/source",
                "data/wpt",
            ])
            .current_dir(paths::workspace_root()),
    )
}

/// Downloads the Common Crawl WARC segment if it is missing or truncated.
pub fn warc() -> Result<(), String> {
    let dir = paths::warc_dir();
    let path = dir.join(WARC_NAME);
    if path.metadata().is_ok_and(|meta| meta.len() == WARC_SIZE) {
        println!("warc: {}", path.display());
        return Ok(());
    }
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
    let part = path.with_extension("gz.part");
    println!("warc: downloading {WARC_NAME}");
    exec(Command::new("curl").args([
        "-fL",
        "-C",
        "-",
        "--retry",
        "6",
        "--retry-delay",
        "2",
        "-o",
        part.to_str().expect("utf-8 path"),
        WARC_URL,
    ]))?;
    let size = part
        .metadata()
        .map_err(|err| format!("{}: {err}", part.display()))?
        .len();
    if size != WARC_SIZE {
        return Err(format!(
            "warc size mismatch: expected {WARC_SIZE}, got {size}"
        ));
    }
    fs::rename(&part, &path).map_err(|err| format!("rename {}: {err}", part.display()))?;
    Ok(())
}

/// Extracts a bounded SWDE working set into `data/swde/html`.
pub fn swde() -> Result<(), String> {
    let html = paths::swde_html_dir();
    if html.is_dir() && count_htm(&html)? > 0 {
        println!("swde: {}", html.display());
        return Ok(());
    }
    let source = paths::swde_source_dir();
    if !source.is_dir() {
        return Err(format!(
            "{} missing; run `cargo xtask data` after initializing the submodule",
            source.display()
        ));
    }
    let archives = archives(&source)?;
    if archives.is_empty() {
        return Err(format!("no .7z archives under {}", source.display()));
    }
    let mut candidates = Vec::new();
    for archive in &archives {
        candidates.extend(list_archive(archive)?);
    }
    if candidates.is_empty() {
        return Err("no .htm files in SWDE archives".to_string());
    }
    let selected = select(candidates, SamplePolicy::default().quota(VERTICALS.len()));
    if selected.is_empty() {
        return Err("SWDE sample is empty".to_string());
    }

    let root = paths::data_dir();
    let tmp = root.join(SWDE_TMP);
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(|err| format!("remove {}: {err}", tmp.display()))?;
    }
    fs::create_dir_all(&tmp).map_err(|err| format!("create {}: {err}", tmp.display()))?;
    let mut by_archive: BTreeMap<PathBuf, Vec<&Entry>> = BTreeMap::new();
    for entry in &selected {
        by_archive
            .entry(entry.archive.clone())
            .or_default()
            .push(entry);
    }
    for (archive, entries) in &by_archive {
        extract(archive, entries, &root.join(SWDE_LIST), &tmp)?;
    }

    if html.exists() {
        fs::remove_dir_all(&html).map_err(|err| format!("remove {}: {err}", html.display()))?;
    }
    fs::create_dir_all(html.parent().expect("data/swde"))
        .map_err(|err| format!("create data/swde: {err}"))?;
    fs::rename(&tmp, &html).map_err(|err| format!("rename {}: {err}", tmp.display()))?;
    let _ = fs::remove_file(root.join(SWDE_LIST));
    println!(
        "swde: {} documents in {}",
        count_htm(&html)?,
        html.display()
    );
    Ok(())
}

/// Removes the downloaded and derived inputs.
pub fn clear() -> Result<(), String> {
    let root = paths::data_dir();
    for path in ["commoncrawl", "swde/html", SWDE_TMP, SWDE_LIST] {
        let path = root.join(path);
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
        } else if path.exists() {
            fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
        }
    }
    println!("removed downloaded and derived data");
    Ok(())
}

fn exec(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|err| format!("spawn {command:?}: {err}"))?;
    if !status.success() {
        return Err(format!("command failed: {status}"));
    }
    Ok(())
}

fn archives(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
        let path = entry
            .map_err(|err| format!("read {}: {err}", dir.display()))?
            .path();
        if path.extension().is_some_and(|ext| ext == "7z") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn list_archive(archive: &Path) -> Result<Vec<Entry>, String> {
    let output = Command::new("7z")
        .args(["l", "-slt"])
        .arg(archive)
        .output()
        .map_err(|err| format!("run 7z (install p7zip-full): {err}"))?;
    if !output.status.success() {
        return Err(format!("7z l failed for {}", archive.display()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut in_entries = false;
    let mut current: Option<(String, usize)> = None;
    let mut out = Vec::new();
    for line in text.lines() {
        if line == "----------" {
            in_entries = true;
            continue;
        }
        if !in_entries {
            continue;
        }
        if let Some(path) = line.strip_prefix("Path = ") {
            if let Some((path, size)) = current.take() {
                push_entry(archive, path, size, &mut out);
            }
            current = Some((path.replace('\\', "/"), 0));
        } else if let Some(size) = line.strip_prefix("Size = ")
            && let Some((_, slot)) = current.as_mut()
        {
            *slot = size.parse().unwrap_or(0);
        }
    }
    if let Some((path, size)) = current {
        push_entry(archive, path, size, &mut out);
    }
    Ok(out)
}

fn push_entry(archive: &Path, path: String, size: usize, out: &mut Vec<Entry>) {
    if size == 0 || !path.ends_with(".htm") {
        return;
    }
    let Some(vertical) = VERTICALS.iter().find(|v| path.starts_with(**v)) else {
        return;
    };
    out.push(Entry {
        archive: archive.to_path_buf(),
        path,
        size,
        vertical,
    });
}

fn select(mut entries: Vec<Entry>, quota: crate::Quota) -> Vec<Entry> {
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups: Vec<Vec<Entry>> = (0..VERTICALS.len()).map(|_| Vec::new()).collect();
    for entry in entries {
        let slot = VERTICALS
            .iter()
            .position(|v| *v == entry.vertical)
            .expect("known vertical");
        groups[slot].push(entry);
    }
    let mut out = Vec::new();
    for group in groups {
        if group.is_empty() {
            continue;
        }
        let step = group.len().div_ceil(quota.docs).max(1);
        let mut bytes = 0usize;
        for (taken, entry) in group.into_iter().step_by(step).enumerate() {
            if taken >= quota.docs {
                break;
            }
            if bytes.saturating_add(entry.size) > quota.bytes && taken > 0 {
                break;
            }
            bytes += entry.size;
            out.push(entry);
        }
    }
    out
}

fn extract(archive: &Path, entries: &[&Entry], list: &Path, out: &Path) -> Result<(), String> {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.path);
        text.push('\n');
    }
    fs::write(list, text).map_err(|err| format!("write {}: {err}", list.display()))?;
    let include = format!("-i@{}", list.display());
    let output = format!("-o{}", out.display());
    exec(
        Command::new("7z")
            .args(["x", "-y", &output])
            .arg(archive)
            .arg(include),
    )
}

fn count_htm(dir: &Path) -> Result<usize, String> {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
            let path = entry
                .map_err(|err| format!("read {}: {err}", dir.display()))?
                .path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "htm") {
                count += 1;
            }
        }
    }
    Ok(count)
}
