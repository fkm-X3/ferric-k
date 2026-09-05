use crate::image;
use crate::platform;
use crate::qmp::QmpClient;
use crate::steps;
use crate::util;
use clap::Args;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const BOOT_MARKER: &str = "BOOT OK";
const FRAMEBUFFER_MARKER: &str = "FRAMEBUFFER OK";
const CONSOLE_MARKER: &str = "Hello from Ferric-K!";
const UPTIME_OK_MARKER: &str = "UPTIME OK";
const INPUT_OK_MARKER: &str = "INPUT OK";
/// Key the smoke injects on each arch through its native input path.
const INJECT_KEY: &str = "a";
/// Serial bytes kept in the reader's rolling window for marker detection.
const WINDOW_MAX: usize = 64;

/// Exit code QEMU reports for a successful input echo. x86_64's isa-debug-exit
/// value (STATUS_INPUT_ECHO<<1)|1 = 0x103 = 259, but POSIX waitpid truncates
/// child status to 8 bits, so Linux CI sees 3; Windows reports the full 259.
#[cfg(windows)]
const X64_INPUT_ECHO_EXIT: i32 = (0x81 << 1) | 1;
#[cfg(not(windows))]
const X64_INPUT_ECHO_EXIT: i32 = ((0x81 << 1) | 1) & 0xFF;

/// Raw semihosting pass-through, already < 256, so identity on every platform.
const ARM64_INPUT_ECHO_EXIT: i32 = 0x81; // STATUS_INPUT_ECHO -> 129

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

struct MachineSpec {
    args: Vec<String>,
    expected_exit: i32,
    qmp_port: Option<u16>,
    serial_port: Option<u16>,
}

/// Builds the QEMU command line; `expected_exit` is only meaningful with
/// `--smoke`, and the input-injection sockets are only wired in that mode.
fn machine_spec(args: &RunArgs, image_path: &Path, repo_root: &Path) -> Result<MachineSpec, String> {
    if args.arch == "x64" {
        let qmp_port = if args.smoke {
            Some(ephemeral_port()?)
        } else {
            None
        };
        let mut cmd: Vec<String> = vec![
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
        ];
        if let Some(port) = qmp_port {
            cmd.extend(["-qmp".into(), format!("tcp:127.0.0.1:{port},server=on,wait=off")]);
        }
        Ok(MachineSpec {
            args: cmd,
            expected_exit: if args.smoke {
                X64_INPUT_ECHO_EXIT
            } else {
                0
            },
            qmp_port,
            serial_port: None,
        })
    } else {
        let firmware = repo_root.join("third_party/firmware/edk2-aarch64-code.fd");
        if !firmware.is_file() {
            return Err(format!(
                "aarch64 UEFI firmware missing at {}. Run: cargo xtask bootstrap",
                firmware.display()
            ));
        }
        let serial_port = if args.smoke {
            Some(ephemeral_port()?)
        } else {
            None
        };
        let mut cmd: Vec<String> = vec![
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
            format!("if=virtio,format=raw,file={}", image_path.display()),
            "-device".into(),
            "ramfb".into(),
        ];
        match serial_port {
            // Smoke drives uart0 through a socket so it can read the serial
            // stream and type into the kernel's PL011 at the same time.
            Some(port) => cmd.extend([
                "-chardev".into(),
                format!("socket,id=charserial0,host=127.0.0.1,port={port},server=on,wait=off"),
                "-serial".into(),
                "chardev:charserial0".into(),
            ]),
            None => cmd.extend(["-serial".into(), "stdio".into()]),
        }
        Ok(MachineSpec {
            args: cmd,
            expected_exit: if args.smoke {
                ARM64_INPUT_ECHO_EXIT
            } else {
                0
            },
            qmp_port: None,
            serial_port,
        })
    }
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

    let mut spec = machine_spec(&args, &image_path, repo_root)?;

    if !args.smoke {
        steps::note("booting QEMU (interactive; close window or Ctrl-C to stop)");
        let status = std::process::Command::new(qemu)
            .args(&spec.args)
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

    spec.args.extend(["-display".into(), "none".into(), "-no-reboot".into()]);

    let child_stdout = std::fs::File::create(&stdout_log)
        .map_err(|e| format!("cannot create stdout log: {e}"))?;
    let child_stderr = std::fs::File::create(&stderr_log)
        .map_err(|e| format!("cannot create stderr log: {e}"))?;
    let mut child = std::process::Command::new(qemu)
        .args(&spec.args)
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::from(child_stderr))
        .spawn()
        .map_err(|e| format!("failed to spawn {qemu}: {e}"))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(args.smoke_timeout_sec);
    let status = run_with_injection(&mut child, &spec, &stdout_log, deadline)?;

    assert_smoke(status.code().unwrap_or(-1), spec.expected_exit, &stdout_log)
}

/// Waits for the child to boot, injects one key down the architecture's
/// native input path, then waits for the exit.
fn run_with_injection(
    child: &mut Child,
    spec: &MachineSpec,
    stdout_log: &Path,
    deadline: std::time::Instant,
) -> Result<ExitStatus, String> {
    if let Some(port) = spec.qmp_port {
        // x86_64: drive HMP over QMP and `sendkey` once the kernel reaches
        // the console, exactly as a real keyboard would inject set-1 codes.
        let mut qmp = QmpClient::connect(port)?;
        wait_for_exit(child, deadline, stdout_log, |_child| {
            if log_has_marker(stdout_log, CONSOLE_MARKER) {
                qmp.human_monitor_command(&format!("sendkey {INJECT_KEY}"))?;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    } else if let Some(port) = spec.serial_port {
        // aarch64: uart0 is a socket chardev; read it for the console marker
        // and then type a key into the kernel's PL011 RX.
        let stream = connect_retry(port, "serial")?;
        let mut write_side = stream
            .try_clone()
            .map_err(|e| format!("cloning serial socket: {e}"))?;
        let got_console = Arc::new(AtomicBool::new(false));
        let reader = spawn_serial_reader(stream, stdout_log, &got_console)?;
        let status = wait_for_exit(child, deadline, stdout_log, |_child| {
            if got_console.load(Ordering::SeqCst) {
                write_side
                    .write_all(INJECT_KEY.as_bytes())
                    .map_err(|e| format!("writing '{INJECT_KEY}' to serial: {e}"))?;
                Ok(true)
            } else {
                Ok(false)
            }
        });
        reader
            .join()
            .map_err(|_| "serial reader thread panicked".to_string())?;
        status
    } else {
        Err("smoke mode requires an input-injection channel".into())
    }
}

/// Polls the child until it exits or the deadline passes; calls `inject`
/// until one call reports it acted, then leaves it alone.
fn wait_for_exit(
    child: &mut Child,
    deadline: std::time::Instant,
    stdout_log: &Path,
    mut inject: impl FnMut(&mut Child) -> Result<bool, String>,
) -> Result<ExitStatus, String> {
    let mut injected = false;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "kernel produced no serial banner + exit within the timeout (killed QEMU). Serial tail:\n{}",
                tail(stdout_log)
            ));
        }
        if !injected && inject(child)? {
            injected = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn log_has_marker(log: &Path, marker: &str) -> bool {
    std::fs::read_to_string(log)
        .map(|content| content.contains(marker))
        .unwrap_or(false)
}

/// Binds an OS-chosen free port and returns it (the listener is dropped so
/// QEMU can take the address itself).
fn ephemeral_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot bind an ephemeral port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read ephemeral port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// Connects to `127.0.0.1:port`, retrying until QEMU's server is listening.
fn connect_retry(port: u16, what: &str) -> Result<TcpStream, String> {
    let mut last = None;
    for _ in 0..400 {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "smoke never reached the {what} socket on 127.0.0.1:{port} ({})",
        last.map(|e| format!("{e}")).unwrap_or_default()
    ))
}

/// Appends everything the serial socket emits to the smoke log, flagging the
/// console marker as soon as the kernel reaches it.
fn spawn_serial_reader(
    stream: TcpStream,
    log: &Path,
    got_console: &Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    let log = log.to_path_buf();
    let flag = Arc::clone(got_console);
    Ok(std::thread::spawn(move || {
        let mut serial = stream;
        let mut buffer = [0u8; 4096];
        let mut window: Vec<u8> = Vec::new();
        let mut sink = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&log);
        loop {
            let n = match serial.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if let Ok(sink) = sink.as_mut()
                && sink.write_all(&buffer[..n]).is_err()
            {
                break;
            }
            window.extend_from_slice(&buffer[..n]);
            if window.len() > WINDOW_MAX {
                window.drain(..window.len() - WINDOW_MAX);
            }
            if String::from_utf8_lossy(&window).contains(CONSOLE_MARKER) {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(sink) = sink.as_mut() {
            let _ = sink.flush();
        }
    }))
}

fn assert_smoke(code: i32, expected: i32, stdout_log: &Path) -> Result<(), String> {
    let serial = std::fs::read_to_string(stdout_log).unwrap_or_default();
    for marker in [
        BOOT_MARKER,
        FRAMEBUFFER_MARKER,
        CONSOLE_MARKER,
        UPTIME_OK_MARKER,
        INPUT_OK_MARKER,
    ] {
        if !serial.contains(marker) {
            return Err(format!(
                "serial log lacks '{marker}' marker. Serial tail:\n{}",
                tail(stdout_log)
            ));
        }
    }
    if code != expected {
        return Err(format!(
            "QEMU exit code {code}, expected {expected}. Serial tail:\n{}",
            tail(stdout_log)
        ));
    }
    steps::ok(&format!(
        "serial banners + echoed '{INJECT_KEY}' + '{INPUT_OK_MARKER}' asserted + clean exit code {code}"
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