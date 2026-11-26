//! Trigger corpus fetching; the implementation lives in `simdhtml-data`.

use anyhow::{Result, anyhow};

pub fn run(clear: bool) -> Result<()> {
    if clear {
        return step("clear", simdhtml_data::fetch::clear());
    }
    // Submodules first: the SWDE archives and the WPT data live there.
    step("submodules", simdhtml_data::fetch::submodules())?;
    step("warc", simdhtml_data::fetch::warc())?;
    step("swde", simdhtml_data::fetch::swde())
}

fn step(name: &str, result: Result<(), String>) -> Result<()> {
    result.map_err(|err| anyhow!("{name}: {err}"))
}
