//! Read and write `.replay` files. A `.replay` is a single pretty-printed JSON
//! document of a [`Run`] — diff-friendly in git and trivially inspectable.

use crate::model::{Run, FORMAT_VERSION};
use anyhow::{anyhow, Context, Result};
use std::path::Path;

pub fn write(run: &Run, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn read(path: &Path) -> Result<Run> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let run: Run = serde_json::from_str(&text)
        .with_context(|| format!("parsing {} as a .replay file", path.display()))?;
    if run.format > FORMAT_VERSION {
        return Err(anyhow!(
            "{} was written by a newer agentreplay (format {}); upgrade the tool",
            path.display(),
            run.format
        ));
    }
    Ok(run)
}
