use crate::util::{checked, run};
use std::path::Path;

pub fn channel_from_toolchain(repo_root: &Path) -> Result<String, String> {
    let toml = std::fs::read_to_string(repo_root.join("rust-toolchain.toml"))
        .map_err(|e| format!("cannot read rust-toolchain.toml: {e}"))?;
    for line in toml.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("channel")
            && let Some(eq) = rest.find('=')
        {
            let v = rest[eq + 1..].trim().trim_matches('"');
            if !v.is_empty() {
                return Ok(v.to_string());
            }
        }
    }
    Err("could not parse channel from rust-toolchain.toml".into())
}

/// Ensure the pinned nightly toolchain and its components are installed.
pub fn ensure_toolchain(repo_root: &Path) -> Result<(), String> {
    let channel = channel_from_toolchain(repo_root)?;

    let list =
        run("rustup", &["toolchain", "list"]).map_err(|e| format!("failed to run rustup: {e}"))?;
    let installed = String::from_utf8_lossy(&list.stdout).to_string();
    if installed.contains(&channel) {
        crate::steps::ok(&format!("toolchain installed: {channel}"));
    } else {
        crate::steps::note(&format!("installing {channel} ..."));
        checked(
            "rustup",
            &["toolchain", "install", &channel, "--no-self-update"],
            "install toolchain",
        )?;
        crate::steps::ok(&format!("toolchain installed: {channel}"));
    }

    let comps = run("rustup", &["component", "list", "--toolchain", &channel])
        .map_err(|e| format!("failed to list rustup components: {e}"))?;
    let have = String::from_utf8_lossy(&comps.stdout).to_string();

    for comp in ["rust-src", "llvm-tools-preview", "rustfmt", "clippy"] {
        // rustup may report llvm-tools-preview under its real name llvm-tools.
        let matches = if comp == "llvm-tools-preview" {
            have.lines()
                .any(|l| l.starts_with("llvm-tools") && l.contains("(installed)"))
        } else {
            have.lines()
                .any(|l| l.starts_with(comp) && l.contains("(installed)"))
        };
        if matches {
            crate::steps::ok(&format!("component installed: {comp}"));
        } else {
            crate::steps::note(&format!("adding component {comp} ..."));
            checked(
                "rustup",
                &["component", "add", "--toolchain", &channel, comp],
                &format!("add component {comp}"),
            )?;
            crate::steps::ok(&format!("component installed: {comp}"));
        }
    }
    Ok(())
}
