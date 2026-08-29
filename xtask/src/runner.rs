use crate::image;
use crate::platform;
use crate::steps;
use crate::util;
use clap::Args;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const BOOT_MARKER: &str = "BOOT OK";
const FRAMEBUFFER_MARKER: &str = "FRAMEBUFFER OK";

#[derive(Args)]
pub struct RunArgs {
    /// x64 or arm64.
    #[arg(long, value_parser = ["x64", "arm64"], default_value = "x64")]
    pub arch: String,
    /// Headless smoke boot with serial-banner + exit-code assertions.
    #[arg(long)]
    pub smoke: bool,
    /// Image path (default build/ferric.img).
    #[arg(long)]
    pub image_path: Option<String>,
    /// Smoke timeout in seconds.
    #[arg(long, default_value_t = 120)]
    pub smoke_timeout_sec: u64,
}

pub fn run(repo_root: &Path, args: RunArgs) -> Result<(), String> {
    let qemu = if args.arch == "x64" {
        platform::QEMU_X64
    } else {
        platform::QEMU_ARM64
    };
    util::find(qemu).map_err(|e| format!("{e} (run: cargo xtask bootstrap)"))?;

    let image_path = repo_root.join(
        args.image_path
            .clone()
            .unwrap_or_else(|| "build/ferric.img".into()),
    );
    if !image_path.is_file() {
        steps::note("image missing, building it");
        let img_args = image::ImageArgs {
            image_path: image_path.display().to_string(),
            size_mb: 64,
        };
        image::run(repo_root, img_args)?;
    }

    let (mut machine, expected_exit_code): (Vec<String>, i32) = if args.arch == "x64" {
        (
            vec![
                "-M".into(),
                "q35".into(),
                "-m".into(),
                "2G".into(),
                "-device".into(),
                "isa-debug-exit,iobase=0x501,iosize=0x2".into(),
                "-hda".into(),
                image_path.display().to_string(),
                "-serial".into(),
                "stdio".into(),
            ],
            (0x10 << 1) | 1, // STATUS_BOOT_OK -> 33
        )
    } else {
        let firmware = repo_root.join("third_party/firmware/edk2-aarch64-code.fd");
        if !firmware.is_file() {
            return Err(format!(
                "aarch64 UEFI firmware missing at {}. Run: cargo xtask bootstrap",
                firmware.display()
            ));
        }
        (
            vec![
                "-M".into(),
                "virt".into(),
                "-cpu".into(),
                "cortex-a72".into(),
                "-m".into(),
                "2G".into(),
                "-semihosting-config".into(),
                "enable=on,target=native".into(),
                "-bios".into(),
                firmware.display().to_string(),
                "-drive".into(),
                format!("if=virtio,format=raw,file={}", image_path.display()),
                "-device".into(),
                "ramfb".into(),
                "-serial".into(),
                "stdio".into(),
            ],
            0x10, // STATUS_BOOT_OK -> 16
        )
    };

    if !args.smoke {
        steps::note("booting QEMU (interactive; close window or Ctrl-C to stop)");
        let status = std::process::Command::new(qemu)
            .args(&machine)
            .status()
            .map_err(|e| format!("failed to run {qemu}: {e}"))?;
        println!("QEMU exited with code {}", status.code().unwrap_or(-1));
        return Ok(());
    }

    steps::note("smoke boot (headless)");
    let build_dir = repo_root.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("cannot create {build_dir:?}: {e}"))?;
    let stdout_log = build_dir.join(format!("last-smoke-{}-stdout.log", args.arch));
    let stderr_log = build_dir.join(format!("last-smoke-{}-stderr.log", args.arch));

    machine.extend(["-display".into(), "none".into(), "-no-reboot".into()]);

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

    let deadline = std::time::Instant::now() + Duration::from_secs(args.smoke_timeout_sec);
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            break st;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let serial = tail(&stdout_log);
            return Err(format!(
                "kernel produced no serial banner + exit within {}s (killed QEMU). Serial tail:\n{}",
                args.smoke_timeout_sec, serial
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let serial = std::fs::read_to_string(&stdout_log).unwrap_or_default();
    if !serial.contains(BOOT_MARKER) {
        return Err(format!(
            "serial log lacks '{BOOT_MARKER}' marker. Serial tail:\n{}",
            tail(&stdout_log)
        ));
    }
    if !serial.contains(FRAMEBUFFER_MARKER) {
        return Err(format!(
            "serial log lacks '{FRAMEBUFFER_MARKER}' marker. Serial tail:\n{}",
            tail(&stdout_log)
        ));
    }
    let code = status.code().unwrap_or(-1);
    if code != expected_exit_code {
        return Err(format!(
            "QEMU exit code {code}, expected {expected_exit_code}. Serial tail:\n{}",
            tail(&stdout_log)
        ));
    }
    steps::ok(&format!(
        "serial banners '{BOOT_MARKER}' + '{FRAMEBUFFER_MARKER}' asserted + clean exit code {code}"
    ));
    println!("SMOKE PASSED");
    Ok(())
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
