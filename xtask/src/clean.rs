use crate::steps;
use clap::Args;
use std::path::Path;

/// Directories removed by `cargo xtask clean` — the cargo build cache
/// (target/) plus the assembled dual-arch images (build/).
const CLEAN_DIRS: [&str; 2] = ["target", "build"];

#[derive(Args)]
pub struct CleanArgs {}

pub fn run(repo_root: &Path, _args: CleanArgs) -> Result<(), String> {
    for dir in CLEAN_DIRS {
        let p = repo_root.join(dir);
        if !p.exists() {
            steps::note(&format!("{dir}/ not present, skipping"));
            continue;
        }
        steps::step(&format!("remove {dir}/"));
        std::fs::remove_dir_all(&p).map_err(|e| format!("cannot remove {}: {e}", p.display()))?;
        steps::ok(&format!("removed {}", p.display()));
    }
    println!("\nCache cleared.");
    Ok(())
}
