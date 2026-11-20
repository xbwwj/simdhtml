//! Export HTML responses from a Common Crawl WARC into `data/commoncrawl/html`.
//!
//! This is the Rust replacement for the old `warcio` Python extractor.  The
//! `warc` crate does the WARC framing/gzip; this binary parses the HTTP
//! response inside each `WARC-Type: response` record, removes transfer/content
//! encodings, and writes one `.html` file plus `manifest.jsonl`.
//!
//! ```text
//! cargo run --release -p simdhtml-bench --bin cc-export -- \
//!   --warc data/commoncrawl/warc/CC-MAIN-....warc.gz \
//!   --out data/commoncrawl/html
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::{GzDecoder, ZlibDecoder};
use serde::Serialize;
use warc::{RecordType, WarcHeader, WarcReader};

const MIB: usize = 1024 * 1024;

#[derive(Serialize)]
struct ManifestRow {
    file: String,
    url: Option<String>,
    date: Option<String>,
    warc_record_id: Option<String>,
    http_status: String,
    content_type: String,
    size: u64,
    warc_truncated: Option<String>,
}

#[derive(Serialize)]
struct Summary {
    source_warc: String,
    records_seen: u64,
    response_records: u64,
    html_candidates: u64,
    html_files: u64,
    html_bytes: u64,
}

struct Args {
    warc: PathBuf,
    out: PathBuf,
    force: bool,
    limit: Option<u64>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("[error] {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if args.out.join("_SUCCESS").is_file() && !args.force {
        println!(
            "[skip] HTML dataset already extracted: {} (pass --force to rebuild)",
            args.out.display()
        );
        return Ok(());
    }

    let staging = args.out.with_extension("tmp");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|err| format!("{}: {err}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|err| format!("{}: {err}", staging.display()))?;

    let mut summary = Summary {
        source_warc: args
            .warc
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        records_seen: 0,
        response_records: 0,
        html_candidates: 0,
        html_files: 0,
        html_bytes: 0,
    };
    let manifest_path = staging.join("manifest.jsonl");
    let mut manifest = fs::File::create(&manifest_path)
        .map_err(|err| format!("{}: {err}", manifest_path.display()))?;

    let reader = WarcReader::from_path_gzip(&args.warc)
        .map_err(|err| format!("{}: {err}", args.warc.display()))?;

    println!("[extract] Reading {}", args.warc.display());
    for record in reader.iter_records() {
        let record = record.map_err(|err| format!("WARC record: {err}"))?;
        summary.records_seen += 1;
        if record.warc_type() != &RecordType::Response {
            continue;
        }
        summary.response_records += 1;
        if let Some(limit) = args.limit
            && summary.html_files >= limit
        {
            break;
        }

        let parsed = match parse_http(record.body()) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("[warn] HTTP parse: {err}");
                continue;
            }
        };
        if parsed.status != "200" {
            continue;
        }
        let content_type = parsed
            .headers
            .get("content-type")
            .map_or("", String::as_str)
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if content_type != "text/html" && content_type != "application/xhtml+xml" {
            continue;
        }
        summary.html_candidates += 1;

        let payload = match decode_payload(&parsed.headers, parsed.payload) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!("[warn] payload decode: {err}");
                continue;
            }
        };
        if payload.is_empty() {
            continue;
        }

        let filename = format!("{:06}.html", summary.html_files);
        let path = staging.join(&filename);
        let mut file =
            fs::File::create(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        file.write_all(&payload)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        summary.html_files += 1;
        summary.html_bytes += payload.len() as u64;

        let row = ManifestRow {
            file: filename,
            url: record
                .header(WarcHeader::TargetURI)
                .map(|value| value.into_owned()),
            date: record
                .header(WarcHeader::Date)
                .map(|value| value.into_owned()),
            warc_record_id: record
                .header(WarcHeader::RecordID)
                .map(|value| value.into_owned()),
            http_status: parsed.status.clone(),
            content_type: parsed
                .headers
                .get("content-type")
                .cloned()
                .unwrap_or_default(),
            size: payload.len() as u64,
            warc_truncated: record
                .header(WarcHeader::Truncated)
                .map(|value| value.into_owned()),
        };
        serde_json::to_writer(&mut manifest, &row).map_err(|err| err.to_string())?;
        manifest.write_all(b"\n").map_err(|err| err.to_string())?;

        if summary.html_files.is_multiple_of(500) {
            println!(
                "  extracted {} HTML files, {:.1} MiB",
                summary.html_files,
                summary.html_bytes as f64 / MIB as f64
            );
        }
    }

    manifest.flush().map_err(|err| err.to_string())?;
    let success = staging.join("_SUCCESS");
    let json = serde_json::to_string_pretty(&summary).map_err(|err| err.to_string())?;
    fs::write(&success, format!("{json}\n")).map_err(|err| err.to_string())?;

    if args.out.exists() {
        fs::remove_dir_all(&args.out).map_err(|err| format!("{}: {err}", args.out.display()))?;
    }
    fs::rename(&staging, &args.out).map_err(|err| format!("{}: {err}", args.out.display()))?;

    println!("[done] Extracted {} HTML files", summary.html_files);
    println!(
        "       Total HTML: {:.1} MiB",
        summary.html_bytes as f64 / MIB as f64
    );
    println!(
        "       Manifest:  {}",
        args.out.join("manifest.jsonl").display()
    );
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let data = simdhtml_bench::env::data_dir()
        .unwrap_or_else(|| PathBuf::from("data"))
        .join("commoncrawl");
    let mut warc = None;
    let mut out = data.join("html");
    let mut force = false;
    let mut limit = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--warc" => warc = Some(PathBuf::from(need(&mut args, "--warc")?)),
            "--out" => out = PathBuf::from(need(&mut args, "--out")?),
            "--force" => force = true,
            "--limit" => {
                limit = Some(
                    need(&mut args, "--limit")?
                        .parse()
                        .map_err(|_| "--limit expects a number".to_string())?,
                )
            }
            "-h" | "--help" => {
                println!(
                    "usage: cc-export [--warc PATH] [--out DIR] [--force] [--limit N]\n\
                     default warc: first data/commoncrawl/warc/*.warc.gz"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let warc = match warc {
        Some(path) => path,
        None => find_warc(&data.join("warc"))?,
    };
    Ok(Args {
        warc,
        out,
        force,
        limit,
    })
}

fn need(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn find_warc(dir: &Path) -> Result<PathBuf, String> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gz"))
        .collect();
    files.sort();
    files
        .into_iter()
        .next()
        .ok_or_else(|| format!("no .warc.gz under {}", dir.display()))
}

struct Http<'a> {
    status: String,
    headers: HashMap<String, String>,
    payload: &'a [u8],
}

fn parse_http(body: &[u8]) -> Result<Http<'_>, String> {
    let split = body
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no HTTP header terminator")?;
    let head = &body[..split];
    let payload = &body[split + 4..];
    let mut lines = head.split(|byte| *byte == b'\n');
    let status_line = lines.next().ok_or("no HTTP status line")?;
    let status = String::from_utf8_lossy(status_line)
        .split_whitespace()
        .nth(1)
        .ok_or("no HTTP status code")?
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let name = String::from_utf8_lossy(&line[..colon])
            .trim()
            .to_ascii_lowercase();
        let value = String::from_utf8_lossy(&line[colon + 1..])
            .trim()
            .to_string();
        headers.insert(name, value);
    }
    Ok(Http {
        status,
        headers,
        payload,
    })
}

fn decode_payload(headers: &HashMap<String, String>, payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut data = payload.to_vec();
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        data = decode_chunked(&data)?;
    }
    if let Some(encoding) = headers.get("content-encoding") {
        for coding in encoding.split(',').map(str::trim).rev() {
            data = match coding.to_ascii_lowercase().as_str() {
                "" | "identity" => data,
                "gzip" | "x-gzip" => {
                    let mut out = Vec::new();
                    GzDecoder::new(data.as_slice())
                        .read_to_end(&mut out)
                        .map_err(|err| format!("gzip: {err}"))?;
                    out
                }
                "deflate" => {
                    let mut out = Vec::new();
                    ZlibDecoder::new(data.as_slice())
                        .read_to_end(&mut out)
                        .map_err(|err| format!("deflate: {err}"))?;
                    out
                }
                other => return Err(format!("unsupported content-encoding {other:?}")),
            };
        }
    }
    Ok(data)
}

fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    let mut rest = data;
    loop {
        let Some(end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Err("chunk size without CRLF".to_string());
        };
        let size_text = std::str::from_utf8(&rest[..end])
            .map_err(|_| "non-UTF8 chunk size".to_string())?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size + 2 {
            return Err("truncated chunk".to_string());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}
