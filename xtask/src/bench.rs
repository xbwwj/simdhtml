use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{exec, root};

const ALL: [&str; 7] = [
    "markup",
    "parse",
    "classify",
    "select",
    "parse_compare",
    "extract",
    "python_compare",
];
const QUICK: [&str; 4] = ["markup", "parse", "select", "extract"];
const COMPARE: [&str; 2] = ["extract", "python_compare"];

pub fn run(quick: bool, compare: bool, targets: &[String]) -> Result<()> {
    let root = root();
    let names: Vec<&str> = if !targets.is_empty() {
        targets.iter().map(String::as_str).collect()
    } else if compare {
        COMPARE.to_vec()
    } else if quick {
        QUICK.to_vec()
    } else {
        ALL.to_vec()
    };

    for name in names {
        let mut features = Vec::new();
        match name {
            "parse_compare" | "extract" => features.push("external"),
            "python_compare" => features.push("python"),
            _ => {}
        }
        if quick || compare {
            features.push("quick");
        }

        let mut command = Command::new("cargo");
        command
            .args(["bench", "-p", "simdhtml-bench", "--bench", name])
            .current_dir(&root);
        if !features.is_empty() {
            command.arg("--features").arg(features.join(","));
        }
        command.arg("--");
        if quick {
            command.args(["--quick", "--noplot"]);
        }
        if name == "python_compare" {
            python_env(&root, &mut command)?;
        }
        exec(&mut command)?;
    }
    Ok(())
}

fn python_env(root: &Path, command: &mut Command) -> Result<()> {
    if !root.join(".venv").is_dir() {
        exec(
            Command::new("uv")
                .args(["sync", "--frozen", "--group", "bench"])
                .current_dir(root),
        )?;
    }
    let python = root.join(".venv/bin/python");
    if !python.exists() {
        bail!("{} missing; run `uv sync --group bench`", python.display());
    }
    let site = site_packages(&root.join(".venv"))?;
    command.env("PYO3_PYTHON", &python).env("PYTHONPATH", site);
    Ok(())
}

fn site_packages(venv: &Path) -> Result<PathBuf> {
    let lib = venv.join("lib");
    for entry in fs::read_dir(&lib).with_context(|| format!("read {}", lib.display()))? {
        let path = entry?.path().join("site-packages");
        if path.is_dir() {
            return Ok(path);
        }
    }
    bail!("no site-packages under {}", lib.display())
}
