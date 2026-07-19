//! CLI tools for arcagrad plugin repositories.
//!
//! Generates a repository index from a directory of `.wasm` plugins.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str);
    if cmd != Some("generate-index") {
        eprintln!(
            "usage: arca-plugin generate-index --dir <dir> [--base-url <url>] [--name <name>] [--out <file>]"
        );
        std::process::exit(2);
    }

    let mut dir: Option<PathBuf> = None;
    let mut base_url: Option<String> = None;
    let mut name: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let rest = &args[2..];
    let mut i = 0;
    while i < rest.len() {
        let flag = rest[i].as_str();
        let value = || {
            rest.get(i + 1)
                .cloned()
                .context(format!("{flag} needs a value"))
        };
        match flag {
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--base-url" => base_url = Some(value()?),
            "--name" => name = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            other => bail!("unknown flag {other}"),
        }
        i += 2;
    }
    let dir = dir.context("--dir is required")?;

    let rt = tokio::runtime::Runtime::new()?;
    let index = arcagrad::plugins::plugin_index::generate_from_dir(
        &dir,
        base_url.as_deref(),
        name,
        rt.handle().clone(),
    )?;
    let json = arcagrad::plugins::plugin_index::to_json(&index)?;
    match out {
        Some(path) => {
            std::fs::write(&path, &json).with_context(|| format!("writing {}", path.display()))?;
            eprintln!(
                "wrote {} plugin(s) to {}",
                index.plugins.len(),
                path.display()
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}
