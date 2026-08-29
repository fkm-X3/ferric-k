use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};

pub fn repo_root() -> PathBuf {
    // xtask runs from the workspace root via the cargo alias, or from anywhere
    // inside the repo. Walk up from CARGO_MANIFEST_DIR/.. to find Cargo.toml.
    let mut dir = env::current_dir().expect("cannot determine cwd");
    loop {
        if dir.join("Cargo.toml").exists() {
            return dir;
        }
        if !dir.pop() {
            panic!("could not locate repo root (no Cargo.toml in any parent)");
        }
    }
}

/// Returns an error mentioning the executable when it cannot be spawned, which
/// covers the "not on PATH" case (NotFound).
pub fn find(program: &str) -> Result<PathBuf, String> {
    let probe = if cfg!(windows) {
        program.to_string() + ".exe"
    } else {
        program.to_string()
    };
    if let Ok(paths) = env::var("PATH") {
        for dir in env::split_paths(&paths) {
            let cand = dir.join(&probe);
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    Err(format!(
        "'{}' not found on PATH. Run: cargo xtask bootstrap",
        program
    ))
}

pub fn run(program: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(program).args(args).output()
}

pub fn checked(program: &str, args: &[&str], what: &str) -> Result<(), String> {
    let out = run(program, args).map_err(|e| format!("failed to run {program}: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let tail = if msg.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            msg.to_string()
        };
        return Err(format!("{what} failed ({program})\n{}", tail.trim_end()));
    }
    Ok(())
}
