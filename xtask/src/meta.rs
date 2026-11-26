//! Machine and toolchain description for the benchmark figures.
//!
//! `xtask bench` snapshots it to `target/criterion/env.json`; `xtask plot`
//! falls back to probing the current machine when the snapshot is missing.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

pub fn capture() -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "rustc": rustc(),
        "cpu": cpu(),
        "isa": isa(),
    })
}

pub fn write(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(&capture()).expect("json"))
}

pub fn load(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(capture)
}

/// One line for the figure header.
pub fn line(env: &Value) -> String {
    let field = |key: &str| env.get(key).and_then(Value::as_str).unwrap_or("?");
    format!(
        "simdsel {} · Rust v{} release · {} ({}, single core)",
        field("version"),
        field("rustc"),
        field("cpu"),
        field("isa")
    )
}

fn rustc() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let Ok(output) = Command::new(rustc).arg("--version").output() else {
        return "?".to_string();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().nth(1).unwrap_or("?").to_string()
}

fn cpu() -> String {
    if let Ok(text) = fs::read_to_string("/proc/cpuinfo") {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(':')
                && key.trim() == "model name"
            {
                return tidy(value);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
    {
        let value = String::from_utf8_lossy(&output.stdout);
        if !value.trim().is_empty() {
            return tidy(&value);
        }
    }
    std::env::consts::ARCH.to_string()
}

fn tidy(value: &str) -> String {
    let text = value.trim().trim_end_matches(" Processor").trim();
    if let Some((head, tail)) = text.rsplit_once(' ')
        && tail.starts_with(|c: char| c.is_ascii_digit())
        && tail.ends_with("-Core")
    {
        return head.to_string();
    }
    text.to_string()
}

// Mirrors `simdhtml::simd::detect`: AVX-512 needs `f` and `bw`.
#[cfg(target_arch = "x86_64")]
fn isa() -> String {
    if std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw") {
        "AVX-512".to_string()
    } else if std::is_x86_feature_detected!("avx2") {
        "AVX2".to_string()
    } else {
        "scalar".to_string()
    }
}

#[cfg(target_arch = "aarch64")]
fn isa() -> String {
    "NEON".to_string()
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn isa() -> String {
    "scalar".to_string()
}
