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
use std::thread::JoinHandle;
use std::time::Duration;

const BOOT_MARKER: &str = "BOOT OK";
const FRAMEBUFFER_MARKER: &str = "FRAMEBUFFER OK";
const CONSOLE_MARKER: &str = "Hello from Ferric-K!";
const SLINT_MARKER: &str = "SLINT OK";
const GUI_EXIT_MARKER: &str = "GUI EXIT OK";
const MONITOR_MARKER: &str = "MONITOR OK";
const MONITOR_EXIT_MARKER: &str = "MONITOR EXIT OK";
/// Stall watchdog for the smoke driver: re-sends a command whose effect has
/// not shown up on serial within this window (a dropped key under a loaded
/// runner would otherwise wedge the smoke forever).
const RETRY_GRACE: Duration = Duration::from_secs(10);
const MAX_ATTEMPTS: u32 = 3;
/// Marker the `help` output carries, proving a typed command was dispatched.
const HELP_RESPONSE_MARKER: &str = "power off";
/// Marker the shell prints for a command it does not recognize.
const UNKNOWN_RESPONSE_MARKER: &str = "unknown command";
/// Marker the `uptime` output begins with.
const UPTIME_RESPONSE_MARKER: &str = "Uptime:";
/// Marker the `halt` command prints before powering the machine off.
const HALT_RESPONSE_MARKER: &str = "HALT";

/// Command script the smoke drives: each step waits for its marker on serial,
/// then types the next command (commands carry their own Enter key). The
/// monitor round-trip mirrors the GUI one; its exit marker is distinct so the
/// script cannot advance on the GUI's stale exit line.
const SCRIPT: &[(&str, &str)] = &[
    (CONSOLE_MARKER, "gui\r"),
    (SLINT_MARKER, "\x1B"),
    (GUI_EXIT_MARKER, "monitor\r"),
    (MONITOR_MARKER, "\x1B"),
    (MONITOR_EXIT_MARKER, "help\r"),
    (HELP_RESPONSE_MARKER, "xyz\r"),
    (UNKNOWN_RESPONSE_MARKER, "uptime\r"),
    (UPTIME_RESPONSE_MARKER, "halt\r"),
];

/// Exit code QEMU reports for the shell's `halt` command. x86_64's
/// isa-debug-exit value (STATUS_SHELL_HALT<<1)|1 = 0x105 = 261, but POSIX
/// waitpid truncates child status to 8 bits, so Linux CI sees 5; Windows
/// reports the full 261.
#[cfg(windows)]
const X64_SHELL_HALT_EXIT: i32 = (0x82 << 1) | 1;
#[cfg(not(windows))]
const X64_SHELL_HALT_EXIT: i32 = ((0x82 << 1) | 1) & 0xFF;

/// Raw semihosting pass-through, already < 256, so identity on every platform.
const ARM64_SHELL_HALT_EXIT: i32 = 0x82; // STATUS_SHELL_HALT -> 130

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

/// Fastest accelerator that can actually start a guest on this host (`whpx`
/// under working Hyper-V, `hvf` on macOS, `kvm` on Linux), else `None` for
/// TCG. QEMU rejects accelerator *lists*, so each candidate is booted in
/// minimal form to confirm it survives machine init.
fn detect_accel(qemu: &str) -> Result<Option<String>, String> {
    let out = std::process::Command::new(qemu)
        .args(["-accel", "help"])
        .output()
        .map_err(|e| format!("probing {qemu} accelerators: {e}"))?;
    let text = [out.stdout.clone(), out.stderr.clone()].concat();
    let tokens: Vec<String> = String::from_utf8_lossy(&text)
        .split(|c: char| c.is_whitespace() || c == ',' || c == ':' || c == '/')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    for want in ["whpx", "hvf", "kvm"] {
        if tokens.iter().any(|t| t == want) && probe_accel(qemu, want) {
            return Ok(Some(want.to_owned()));
        }
    }
    Ok(None)
}

/// Starts a paused q35 VM under `accel`; a usable accelerator idles until
/// killed, an unavailable one (e.g. `kvm` with no `/dev/kvm`) exits during
/// startup.
fn probe_accel(qemu: &str, accel: &str) -> bool {
    let Ok(mut child) = std::process::Command::new(qemu)
        .args([
            "-accel",
            accel,
            "-machine",
            "q35",
            "-m",
            "64",
            "-display",
            "none",
            "-nodefaults",
            "-S",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    std::thread::sleep(Duration::from_millis(800));
    let alive = child.try_wait().map(|s| s.is_none()).unwrap_or(false);
    if alive {
        let _ = child.kill();
        let _ = child.wait();
    }
    alive
}

/// Builds the QEMU command line; `expected_exit` is only meaningful with
/// `--smoke`, and the input-injection sockets are only wired in that mode.
fn machine_spec(
    args: &RunArgs,
    image_path: &Path,
    repo_root: &Path,
) -> Result<MachineSpec, String> {
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
            cmd.extend([
                "-qmp".into(),
                format!("tcp:127.0.0.1:{port},server=on,wait=off"),
            ]);
        }
        Ok(MachineSpec {
            args: cmd,
            expected_exit: if args.smoke { X64_SHELL_HALT_EXIT } else { 0 },
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
            expected_exit: if args.smoke { ARM64_SHELL_HALT_EXIT } else { 0 },
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

    // aarch64 guests stay on TCG: no host accel runs cross-architecture guests.
    let mut accel_note = String::new();
    if args.arch == "x64"
        && let Some(accel) = detect_accel(qemu)?
    {
        accel_note = format!(", accel={accel}");
        spec.args.push("-accel".into());
        spec.args.push(accel);
    }

    if !args.smoke {
        steps::note("booting QEMU (interactive; close window or Ctrl-C to stop)");
        let status = std::process::Command::new(qemu)
            .args(&spec.args)
            .status()
            .map_err(|e| format!("failed to run {qemu}: {e}"))?;
        println!("QEMU exited with code {}", status.code().unwrap_or(-1));
        return Ok(());
    }

    steps::note(&format!("smoke boot (headless{accel_note})"));
    let build_dir = repo_root.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("cannot create {build_dir:?}: {e}"))?;
    let stdout_log = build_dir.join(format!("last-smoke-{}-stdout.log", args.arch));
    let stderr_log = build_dir.join(format!("last-smoke-{}-stderr.log", args.arch));

    spec.args
        .extend(["-display".into(), "none".into(), "-no-reboot".into()]);

    let child_stdout =
        std::fs::File::create(&stdout_log).map_err(|e| format!("cannot create stdout log: {e}"))?;
    let child_stderr =
        std::fs::File::create(&stderr_log).map_err(|e| format!("cannot create stderr log: {e}"))?;
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

/// Waits for the child to boot, then drives the command script down the
/// architecture's native input path: waits for each step's prerequisite
/// marker on serial, types that step's command, and finally waits for the
/// halt-induced exit.
fn run_with_injection(
    child: &mut Child,
    spec: &MachineSpec,
    stdout_log: &Path,
    deadline: std::time::Instant,
) -> Result<ExitStatus, String> {
    let (mut qmp, mut write_side, reader) = if let Some(port) = spec.qmp_port {
        // x86_64: drive HMP over QMP and `sendkey`, exactly as a real
        // keyboard would inject set-1 codes.
        (Some(QmpClient::connect(port)?), None, None)
    } else if let Some(port) = spec.serial_port {
        // aarch64: uart0 is a socket chardev; read it into the log and type
        // raw bytes into the PL011 RX.
        let stream = connect_retry(port, "serial")?;
        let write = stream
            .try_clone()
            .map_err(|e| format!("cloning serial socket: {e}"))?;
        let reader = spawn_serial_reader(stream, stdout_log)?;
        (None, Some(write), Some(reader))
    } else {
        return Err("smoke mode requires an input-injection channel".into());
    };

    let mut step = 0usize;
    let mut typed = false;
    let mut typed_at = std::time::Instant::now();
    let mut attempts = 0u32;
    let status = wait_for_exit(child, deadline, stdout_log, |_child| {
        if step >= SCRIPT.len() {
            return Ok(true);
        }
        let (prereq, command) = SCRIPT[step];
        if !log_has_marker(stdout_log, prereq) {
            return Ok(false);
        }
        if !typed {
            send_input(qmp.as_mut(), write_side.as_mut(), command)?;
            typed = true;
            typed_at = std::time::Instant::now();
            attempts = 1;
            return Ok(false);
        }
        if SCRIPT
            .get(step + 1)
            .is_some_and(|(next, _)| log_has_marker(stdout_log, next))
        {
            step += 1;
            typed = false;
            attempts = 0;
            return Ok(step >= SCRIPT.len());
        }
        // Step 0 (gui\r) is never re-sent: a second launch while one is
        // starting is worse than a stall.
        if step > 0
            && attempts < MAX_ATTEMPTS
            && typed_at.elapsed() >= RETRY_GRACE
        {
            send_input(qmp.as_mut(), write_side.as_mut(), command)?;
            attempts += 1;
            typed_at = std::time::Instant::now();
        }
        Ok(false)
    });

    if let Some(reader) = reader {
        reader
            .join()
            .map_err(|_| "serial reader thread panicked".to_string())?;
    }
    status
}

/// Types `command` down the active input channel, pacing keystrokes so the
/// arch controller/FIFO can drain between keys.
fn send_input(
    qmp: Option<&mut QmpClient>,
    serial: Option<&mut TcpStream>,
    command: &str,
) -> Result<(), String> {
    if let Some(qmp) = qmp {
        for c in command.chars() {
            let hmp = match c {
                '\r' => "sendkey ret".to_string(),
                ' ' => "sendkey spc".to_string(),
                '\x1B' => "sendkey esc".to_string(),
                _ => format!("sendkey {c}"),
            };
            qmp.human_monitor_command(&hmp)?;
            std::thread::sleep(Duration::from_millis(20));
        }
    } else if let Some(stream) = serial {
        // PL011 has a 16-byte RX FIFO; small chunks with sleeps keep the
        // polled write from overflowing while the guest drains each byte.
        for chunk in command.as_bytes().chunks(4) {
            stream
                .write_all(chunk)
                .map_err(|e| format!("writing command to serial: {e}"))?;
            std::thread::sleep(Duration::from_millis(25));
        }
    } else {
        return Err("smoke mode requires an input-injection channel".into());
    }
    Ok(())
}

/// Polls the child until it exits or the deadline passes; calls `inject` until
/// it reports no more input is pending.
fn wait_for_exit(
    child: &mut Child,
    deadline: std::time::Instant,
    stdout_log: &Path,
    mut inject: impl FnMut(&mut Child) -> Result<bool, String>,
) -> Result<ExitStatus, String> {
    let started_at = std::time::Instant::now();
    let mut done = false;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "kernel produced no serial banner + exit within the timeout ({}s elapsed, killed QEMU). Serial tail:\n{}",
                started_at.elapsed().as_secs(),
                tail(stdout_log)
            ));
        }
        if !done && inject(child)? {
            done = true;
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

/// Appends everything the serial socket emits to the smoke log.
fn spawn_serial_reader(stream: TcpStream, log: &Path) -> Result<JoinHandle<()>, String> {
    let log = log.to_path_buf();
    Ok(std::thread::spawn(move || {
        let mut serial = stream;
        let mut buffer = [0u8; 4096];
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
        SLINT_MARKER,
        GUI_EXIT_MARKER,
        MONITOR_MARKER,
        MONITOR_EXIT_MARKER,
        HELP_RESPONSE_MARKER,
        UNKNOWN_RESPONSE_MARKER,
        UPTIME_RESPONSE_MARKER,
        HALT_RESPONSE_MARKER,
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
        "serial banners + GUI round-trip + monitor round-trip + shell commands (help/unknown/uptime/halt) dispatched + clean exit code {code}"
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
