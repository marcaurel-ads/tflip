//! Build tasks, run as `cargo xtask <task>` (an alias in
//! `.cargo/config.toml`).
//!
//! - `dist [ace32|ace64]...`: builds the client library for Windows and
//!   copies it to `target/dist` under the names clients load: `ace32.dll`
//!   (32-bit, `i686-pc-windows-gnu`) and `ace64.dll` (64-bit,
//!   `x86_64-pc-windows-gnu`). Cargo names it after the crate,
//!   `tflip_ace.dll`, whatever the target. Without names, builds both.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, anyhow, bail};

/// A client DLL: the name clients load and the target it is built for.
struct Dll {
    name: &'static str,
    target: &'static str,
}

const DLLS: [Dll; 2] = [
    Dll {
        name: "ace32",
        target: "i686-pc-windows-gnu",
    },
    Dll {
        name: "ace64",
        target: "x86_64-pc-windows-gnu",
    },
];

/// The file Cargo builds the client library as.
const BUILT: &str = "tflip_ace.dll";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((task, names)) if task == "dist" => dist(names),
        _ => Err(anyhow!("usage: cargo xtask dist [ace32|ace64]...")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the DLLs named in `names` (all if none) in release mode and
/// copies them to `target/dist`.
fn dist(names: &[String]) -> anyhow::Result<()> {
    let unknown: Vec<&String> = names
        .iter()
        .filter(|name| DLLS.iter().all(|dll| dll.name != *name))
        .collect();
    if !unknown.is_empty() {
        bail!("unknown DLLs {unknown:?}; there are ace32 and ace64");
    }
    let root = workspace_root();
    let dist = root.join("target/dist");
    fs::create_dir_all(&dist).with_context(|| dist.display().to_string())?;
    for dll in DLLS
        .iter()
        .filter(|dll| names.is_empty() || names.iter().any(|n| n == dll.name))
    {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .current_dir(&root)
            .args([
                "build",
                "-p",
                "tflip-ace",
                "--release",
                "--target",
                dll.target,
            ])
            .status()
            .context("cannot run cargo")?;
        if !status.success() {
            bail!("building {} for {} failed", dll.name, dll.target);
        }
        let built = root
            .join("target")
            .join(dll.target)
            .join("release")
            .join(BUILT);
        let installed = dist.join(format!("{}.dll", dll.name));
        fs::copy(&built, &installed).with_context(|| built.display().to_string())?;
        println!("{}", installed.display());
    }
    Ok(())
}

/// The workspace root: the parent of this crate's folder.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
