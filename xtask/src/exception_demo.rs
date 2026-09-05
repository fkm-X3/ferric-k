//! Induced-exception demo: builds both kernels with the `exception-on-boot`
//! feature into a scratch target dir, stages a dual-arch image, and asserts
//! the exception diagnostic markers on serial for x86_64 and aarch64. The
//! exception handler parks the CPU with `hlt`/`wfi` (no QEMU exit), so each
//! boot is killed once its serial log proves the dump rendered.

use crate::image;
use crate::platform;
use crate::steps;
use crate::util;
use clap::Args;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const EXCEPTION_MARKER: &str = "EXCEPTION";
const HALT_MARKER: &str = "KERNEL HALT";

const X64_MARKERS: &[&str] = &[EXCEPTION_MARKER, "vector 0", HALT_MARKER];
const ARM64_MARKERS: &[&str] = &[EXCEPTION_MARKER, "SYNC", HALT_MARKER];

#[derive(Args)]
pub struct ExceptionDemoArgs {
    /// Smoke timeout in seconds.
    #[arg(long, default_value_t = 120)]
    pub smoke_timeout_sec: u64,
}

pub fn run(repo_root: &Path, args: ExceptionDemoArgs) -> Result<(), String> {
    let scratch = repo_root.join("build").join("exception-target");
    for (arch, target) in [("x86_64", "x86_64-ferric"), ("aarch64", "aarch64-ferric")] {
        steps::step(&format!("build exception kernel ({arch})"));
        build_exception_kernel(repo_root, target, &scratch)?;
    }

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

    for arch in ["x64", "arm64"] {
        steps::step(&format!("exception smoke boot ({arch})"));
        boot_and_assert_exception(repo_root, arch, &img, args.smoke_timeout_sec)?;
    }

    println!("\nEXCEPTION DEMO PASSED: induced exception diagnostic rendered + serialized on both arches.");
    Ok(())
}

fn build_exception_kernel(repo_root: &Path, target: &str, target_dir: &Path) -> Result<(), String> {
    let target_json = format!("targets/{target}.json");
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg(&target_json)
        .arg("--features")
        .arg("exception-on-boot")
        .arg("-Zbuild-std=core,compiler_builtins")
        .arg("-Zjson-target-spec")
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(repo_root)
        .status()
        .map_err(|e| format!("failed to run cargo for exception kernel ({target}): {e}"))?;
    if !status.success() {
        return Err(format!("exception kernel build failed ({target})"));
    }
    Ok(())
}

fn boot_and_assert_exception(
    repo_root: &Path,
    arch: &str,
    image: &Path,
    timeout_secs: u64,
) -> Result<(), String> {
    let qemu = if arch == "x64" {
        platform::QEMU_X64
    } else {
        platform::QEMU_ARM64
    };
    util::find(qemu).map_err(|e| format!("{e} (run: cargo xtask bootstrap)"))?;

    let markers: &[&str] = if arch == "x64" {
        X64_MARKERS
    } else {
        ARM64_MARKERS
    };

    let mut machine: Vec<String> = if arch == "x64" {
        vec![
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
    } else {
        let firmware = repo_root.join("third_party/firmware/edk2-aarch64-code.fd");
        if !firmware.is_file() {
            return Err(format!(
                "aarch64 UEFI firmware missing at {}. Run: cargo xtask bootstrap",
                firmware.display()
            ));
        }
        vec![
            "-M".into(),
            "virt,gic-version=2".into(),
            "-cpu".into(),
            "cortex-a72".into(),
            "-m".into(),
            "2G".into(),
            "-semihosting-config".into(),
            "enable=on,target=native".into(),
            "-bios".into(),
            firmware.display().to_string(),
            "-drive".into(),
            format!("if=virtio,format=raw,file={}", image.display()),
            "-device".into(),
            "ramfb".into(),
            "-serial".into(),
            "stdio".into(),
        ]
    };
    machine.extend(["-display".into(), "none".into(), "-no-reboot".into()]);

    let build_dir = repo_root.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("cannot create {build_dir:?}: {e}"))?;
    let stdout_log = build_dir.join(format!("last-exception-smoke-{}-stdout.log", arch));
    let stderr_log = build_dir.join(format!("last-exception-smoke-{}-stderr.log", arch));

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
        if markers.iter().all(|m| content.contains(m)) {
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