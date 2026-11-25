use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use simdhtml_data::{Quota, SamplePolicy};

use crate::{exec, root};

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

struct Entry {
    archive: PathBuf,
    path: String,
    size: usize,
    vertical: &'static str,
}

pub fn run(clear: bool) -> Result<()> {
    let root = root();
    if clear {
        clear_data(&root)?;
        return Ok(());
    }

    submodules(&root)?;
    warc(&root)?;
    swde(&root)?;
    Ok(())
}

fn clear_data(root: &Path) -> Result<()> {
    for path in [
        "data/commoncrawl",
        "data/swde/html",
        "data/swde-html",
        "data/swde-index.tsv",
        "target/xtask-swde-list.txt",
        "target/xtask-swde-tmp",
    ] {
        let path = root.join(path);
        if path.is_dir() {
            fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))?;
        } else if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    println!("removed downloaded and derived data");
    Ok(())
}

fn submodules(root: &Path) -> Result<()> {
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
            .current_dir(root),
    )
}

fn warc(root: &Path) -> Result<()> {
    let dir = root.join("data/commoncrawl/warc");
    let path = dir.join(WARC_NAME);
    if path.metadata().is_ok_and(|meta| meta.len() == WARC_SIZE) {
        println!("warc: {}", path.display());
        return Ok(());
    }
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
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
    let size = part.metadata()?.len();
    if size != WARC_SIZE {
        bail!("warc size mismatch: expected {WARC_SIZE}, got {size}");
    }
    fs::rename(&part, &path)?;
    Ok(())
}

fn swde(root: &Path) -> Result<()> {
    let html = root.join("data/swde/html");
    if html.is_dir() && count_htm(&html)? > 0 {
        println!("swde: {}", html.display());
        return Ok(());
    }
    let source = simdhtml_data::swde_source_dir();
    if !source.is_dir() {
        bail!(
            "{} missing; run `cargo xtask data` after initializing the submodule",
            source.display()
        );
    }
    let archives = archives(&source)?;
    if archives.is_empty() {
        bail!("no .7z archives under {}", source.display());
    }
    let mut candidates = Vec::new();
    for archive in &archives {
        candidates.extend(list_archive(archive)?);
    }
    if candidates.is_empty() {
        bail!("no .htm files in SWDE archives");
    }
    let selected = select(candidates, SamplePolicy::default().quota(VERTICALS.len()));
    if selected.is_empty() {
        bail!("SWDE sample is empty");
    }

    let tmp = root.join("target/xtask-swde-tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;
    let mut by_archive: BTreeMap<PathBuf, Vec<&Entry>> = BTreeMap::new();
    for entry in &selected {
        by_archive
            .entry(entry.archive.clone())
            .or_default()
            .push(entry);
    }
    for (archive, entries) in &by_archive {
        extract(archive, entries, root, &tmp)?;
    }

    if html.exists() {
        fs::remove_dir_all(&html)?;
    }
    fs::create_dir_all(html.parent().expect("data/swde"))?;
    fs::rename(&tmp, &html).with_context(|| format!("rename {}", tmp.display()))?;
    println!(
        "swde: {} documents in {}",
        count_htm(&html)?,
        html.display()
    );
    Ok(())
}

fn archives(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "7z") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn list_archive(archive: &Path) -> Result<Vec<Entry>> {
    let output = Command::new("7z")
        .args(["l", "-slt"])
        .arg(archive)
        .output()
        .with_context(|| "run 7z (install p7zip-full)")?;
    if !output.status.success() {
        bail!("7z l failed for {}", archive.display());
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

fn select(mut entries: Vec<Entry>, quota: Quota) -> Vec<Entry> {
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

fn extract(archive: &Path, entries: &[&Entry], root: &Path, out: &Path) -> Result<()> {
    let list = root.join("target/xtask-swde-list.txt");
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.path);
        text.push('\n');
    }
    fs::write(&list, text)?;
    let include = format!("-i@{}", list.display());
    let output = format!("-o{}", out.display());
    exec(
        Command::new("7z")
            .args(["x", "-y", &output])
            .arg(archive)
            .arg(include),
    )?;
    Ok(())
}

fn count_htm(dir: &Path) -> Result<usize> {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "htm") {
                count += 1;
            }
        }
    }
    Ok(count)
}
