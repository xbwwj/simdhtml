mod bench;
mod data;
mod meta;
mod plot;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Repository tasks")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Fetch raw inputs and build the SWDE working set.
    Data {
        /// Remove downloaded and derived data.
        #[arg(long)]
        clear: bool,
    },
    /// Run criterion benchmarks.
    Bench {
        /// Sampled iteration profile, no plots.
        #[arg(long, conflicts_with = "compare")]
        quick: bool,
        /// Scoreboard sample: extract + python_compare.
        #[arg(long)]
        compare: bool,
        /// Benchmark names to run instead of the profile default.
        targets: Vec<String>,
    },
    /// Draw the performance figures from target/criterion.
    Plot {
        /// Skip engines without a criterion result instead of failing.
        #[arg(long)]
        allow_missing: bool,
    },
    /// Run compatibility tests.
    Compat {
        /// Compare the Python binding against parsel.
        #[arg(long)]
        parsel: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Task::Data { clear } => data::run(clear),
        Task::Bench {
            quick,
            compare,
            targets,
        } => bench::run(quick, compare, &targets),
        Task::Plot { allow_missing } => plot::run(allow_missing),
        Task::Compat { parsel } => {
            if !parsel {
                bail!("select a compatibility target: --parsel");
            }
            run_uv(&[
                "run",
                "--frozen",
                "--group",
                "test",
                "pytest",
                "crates/simdsel-py/tests",
            ])
        }
    }
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Criterion's result directory, resolved the same way criterion does.
pub fn criterion_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRITERION_HOME") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("criterion");
    }
    root().join("target/criterion")
}

pub fn exec(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("spawn {command:?}"))?;
    if !status.success() {
        bail!("command failed: {status}");
    }
    Ok(())
}

fn run_uv(args: &[&str]) -> Result<()> {
    exec(Command::new("uv").args(args).current_dir(root()))
}
