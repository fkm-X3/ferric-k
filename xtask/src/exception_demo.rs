//! Induced-exception demo (x86_64 only, arm64 soon): builds the x86_64
//! kernel with the `exception-on-boot` feature into a scratch target dir,
//! stages a dual-arch image (aarch64 uses a normal build), and asserts the 
//! exception diagnostic markerson serial for x86_64. The exception handler 
//! parks the CPU with `hlt` (no QEMU exit), so the boot is killed once its
//! serial log proves the dump rendered.

use crate::image;
use crate::platform;
use crate::steps;
use crate::util;
use clap::Args;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const EXCEPTION_MARKER: &str = "EXCEPTION";
const VECTOR_MARKER: &str = "vector 0";
const HALT_MARKER: &str = "KERNEL HALT";

#[derive(Args)]
pub struct ExceptionDemoArgs {
    /// Smoke timeout in seconds.
    #[arg(long, default_value_t = 120)]
    pub smoke_timeout_sec: u64,
}

pub fn run(repo_root: &Path, args: ExceptionDemoArgs) -> Result<(), String> {
    let scratch = repo_root.join("build").join("exception-target");
    steps::step("build exception kernel (x86_64)");
    build_exception_kernel(repo_root, "x86_64-ferric", &scratch)?;
    steps::step("build companion kernel (aarch64)");
    build_companion_kernel(repo_root, "aarch64-ferric", &scratch)?;

    let img = repo_root.join("build").join("ferric-exception.img");
    let kernels: &[(&str, &str)] = &[
        (
            "kernel-x86_64.elf",
            "build/exception-target/x86_64-ferric/debug/ferric-kernel",
        ),
        (
            "kernel-aarch64.elf",
            "build/exception-target/aarch64-ferric/debug/ferric-kernel",
        ),
    ];
    image::assemble(repo_root, &img, 64, kernels)?;

    steps::step("exception smoke boot (x64)");
    boot_and_assert_exception(repo_root, &img, args.smoke_timeout_sec)?;

    println!("\nEXCEPTION DEMO PASSED: induced exception diagnostic rendered + serialized on x86_64.");
    Ok(())
}

fn build_kernel(repo_root: &Path, target: &str, features: Option<&str>, target_dir: &Path) -> Result<(), String> {
    let target_json = format!("targets/{target}.json");
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build")
        .arg("--target")
        .arg(&target_json)
        .arg("-Zbuild-std=core,compiler_builtins")
        .arg("-Zjson-target-spec")
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(repo_root);
    if let Some(f) = features {
        cmd.arg("--features").arg(f);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo for kernel ({target}): {e}"))?;
    if !status.success() {
        return Err(format!("kernel build failed ({target})"));
    }
    Ok(())
}

fn build_exception_kernel(repo_root: &Path, target: &str, target_dir: &Path) -> Result<(), String> {
    build_kernel(repo_root, target, Some("exception-on-boot"), target_dir)
}

fn build_companion_kernel(repo_root: &Path, target: &str, target_dir: &Path) -> Result<(), String> {
    build_kernel(repo_root, target, None, target_dir)
}

fn boot_and_assert_exception(
    repo_root: &Path,
    image: &Path,
    timeout_secs: u64,
) -> Result<(), String> {
    let qemu = platform::QEMU_X64;
    util::find(qemu).map_err(|e| format!("{e} (run: cargo xtask bootstrap)"))?;

    let machine: Vec<String> = vec![
        "-M".into(),
        "q35".into(),
        "-m".into(),
        "2G".into(),
        "-device".into(),
        "isa-debug-exit,iobase=0x501,iosize=0x2".into(),
        "-hda".into(),
        image.display().to_string(),
        "-serial".into(),
        "stdio".into(),
    ]
    .into_iter()
    .chain(["-display".into(), "none".into(), "-no-reboot".into()])
    .collect();

    let build_dir = repo_root.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("cannot create {build_dir:?}: {e}"))?;
    let stdout_log = build_dir.join("last-exception-smoke-x64-stdout.log");
    let stderr_log = build_dir.join("last-exception-smoke-x64-stderr.log");

    let stdout =
        std::fs::File::create(&stdout_log).map_err(|e| format!("cannot create stdout log: {e}"))?;
    let stderr =
        std::fs::File::create(&stderr_log).map_err(|e| format!("cannot create stderr log: {e}"))?;
    let mut child = std::process::Command::new(qemu)
        .args(&machine)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| format!("failed to spawn {qemu}: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Some(st) = child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            let serial = tail(&stdout_log);
            return Err(format!(
                "QEMU exited ({}) before the exception marker appeared. Serial tail:\n{}",
                st.code().unwrap_or(-1),
                serial
            ));
        }
        let content = std::fs::read_to_string(&stdout_log).unwrap_or_default();
        if content.contains(EXCEPTION_MARKER)
            && content.contains(VECTOR_MARKER)
            && content.contains(HALT_MARKER)
        {
            let _ = child.kill();
            let _ = child.wait();
            steps::ok("exception diagnostic markers found on serial");
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let serial = tail(&stdout_log);
            return Err(format!(
                "no exception markers within {}s (killed QEMU). Serial tail:\n{}",
                timeout_secs, serial
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn tail(path: &Path) -> String {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(10);
    if lines.is_empty() {
        "<serial empty>".to_string()
    } else {
        lines[start..].join("\n")
    }
}
