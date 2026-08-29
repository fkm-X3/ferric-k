use crate::util::checked;
use std::path::PathBuf;

/// Standard qemu executable names used across the harness.
pub const QEMU_X64: &str = "qemu-system-x86_64";
pub const QEMU_ARM64: &str = "qemu-system-aarch64";
pub const MTOOLS: &[&str] = &["mformat", "mmd", "mcopy", "mdir"];

#[derive(Clone, Copy, PartialEq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
    Other,
}

impl Os {
    pub fn detect() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Other
        }
    }
}

pub struct NativeDeps {
    pub qemu_x64: bool,
    pub qemu_arm64: bool,
    pub mtools: bool,
    pub firmware: Option<PathBuf>,
}

fn have(program: &str) -> bool {
    let probe = if cfg!(windows) {
        program.to_string() + ".exe"
    } else {
        program.to_string()
    };
    std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .any(|d| d.join(&probe).is_file())
}

/// Locate edk2 aarch64 UEFI firmware next to the installed QEMU's share dir.
pub fn find_firmware_from_qemu() -> Option<PathBuf> {
    // For every PATH entry, walk up one level (bin -> prefix) and look in
    // <prefix>/share/qemu for the aarch64 edk2 blob.
    for bin_dir in std::env::split_paths(&std::env::var("PATH").unwrap_or_default()) {
        if bin_dir.join(qemu_arm64()).is_file()
            && let Some(prefix) = bin_dir.parent()
        {
            let share = prefix.join("share").join("qemu");
            for name in ["edk2-aarch64-code.fd", "QEMU_EFI-aarch64.fd"] {
                let cand = share.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn qemu_arm64() -> &'static str {
    if cfg!(windows) {
        "qemu-system-aarch64.exe"
    } else {
        "qemu-system-aarch64"
    }
}

/// Well-known firmware locations per Linux distro's packaged edk2 layout.
pub fn find_firmware_known() -> Option<PathBuf> {
    for cand in [
        "/usr/share/AAVMF/AAVMF_CODE.fd",
        "/usr/share/edk2/aarch64/QEMU_EFI.fd",
        "/usr/share/edk2/aarch64/QEMU_EFI-aarch64.fd",
        "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
    ] {
        let p = std::path::Path::new(cand);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

pub fn scan_native() -> NativeDeps {
    NativeDeps {
        qemu_x64: have(QEMU_X64),
        qemu_arm64: have(QEMU_ARM64),
        mtools: MTOOLS.iter().all(|t| have(t)),
        firmware: find_firmware_from_qemu().or_else(find_firmware_known),
    }
}

/// Human-readable list of the native deps that are missing.
pub fn missing(deps: &NativeDeps) -> Vec<String> {
    let mut v = Vec::new();
    if !deps.qemu_x64 && !deps.qemu_arm64 {
        v.push("QEMU".into());
    }
    if !deps.mtools {
        v.push("mtools".into());
    }
    if deps.firmware.is_none() {
        v.push("edk2 aarch64 firmware".into());
    }
    v
}

/// Install all native build deps using the host's package manager (MSYS2
/// pacman on Windows, Homebrew on macOS, apt/dnf/pacman on Linux).
pub fn install_native() -> Result<(), String> {
    match Os::detect() {
        Os::Windows => install_windows(),
        Os::Macos => install_macos(),
        Os::Linux => install_linux(),
        Os::Other => Err("unsupported host OS for native dependency installation".into()),
    }
}

fn install_windows() -> Result<(), String> {
    let pacman = if have("pacman") {
        "pacman".to_string()
    } else if std::path::Path::new("C:\\msys64\\usr\\bin\\pacman.exe").is_file() {
        "C:\\msys64\\usr\\bin\\pacman.exe".to_string()
    } else {
        return Err("pacman not on PATH. Install MSYS2 UCRT64 and put C:\\msys64\\ucrt64\\bin + C:\\msys64\\usr\\bin on PATH.".into());
    };

    let deps = scan_native();
    if !(deps.qemu_x64 || deps.qemu_arm64) {
        crate::steps::note("installing QEMU via MSYS2 pacman ...");
        checked(
            &pacman,
            &[
                "-S",
                "--needed",
                "--noconfirm",
                "mingw-w64-ucrt-x86_64-qemu",
            ],
            "pacman install qemu",
        )?;
    } else {
        crate::steps::ok("QEMU present");
    }
    if !deps.mtools {
        crate::steps::note("installing mtools via MSYS2 pacman ...");
        checked(
            &pacman,
            &[
                "-S",
                "--needed",
                "--noconfirm",
                "mingw-w64-ucrt-x86_64-mtools",
            ],
            "pacman install mtools",
        )?;
    } else {
        crate::steps::ok("mtools present");
    }
    Ok(())
}

fn install_macos() -> Result<(), String> {
    if !have("brew") {
        return Err("native deps on macOS need Homebrew; install from https://brew.sh".into());
    }
    let deps = scan_native();
    let mut pkgs: Vec<&str> = Vec::new();
    if !(deps.qemu_x64 && deps.qemu_arm64) {
        pkgs.push("qemu");
    }
    if !deps.mtools {
        pkgs.push("mtools");
    }
    if pkgs.is_empty() {
        return Ok(());
    }
    crate::steps::note(&format!("installing via brew: {}", pkgs.join(" ")));
    let mut args = vec!["install"];
    args.extend(pkgs);
    checked("brew", &args, "brew install native deps")?;
    Ok(())
}

/// Returns the package manager binary and the package list for the detected
/// Linux distro, preferring the distro's native manager and falling back to
/// whichever tool happens to be installed.
fn linux_pkg_manager() -> Option<(&'static str, Vec<&'static str>)> {
    let id = std::fs::read_to_string("/etc/os-release")
        .ok()
        .map(|s| s.to_lowercase());
    let is = |frag: &str| id.as_ref().map(|s| s.contains(frag)).unwrap_or(false);

    if is("debian") || is("ubuntu") {
        Some((
            "apt",
            vec![
                "qemu-system-x86",
                "qemu-system-arm",
                "mtools",
                "qemu-efi-aarch64",
            ],
        ))
    } else if is("fedora") || is("rhel") || is("centos") || is("rocky") || is("alma") {
        Some((
            "dnf",
            vec![
                "qemu-system-x86-core",
                "qemu-system-aarch64",
                "mtools",
                "edk2-aarch64",
            ],
        ))
    } else if is("arch") || is("manjaro") || is("endeavouros") {
        Some(("pacman", vec!["qemu", "mtools", "edk2-aarch64"]))
    } else if std::process::Command::new("apt-get")
        .arg("--version")
        .output()
        .is_ok()
    {
        Some((
            "apt",
            vec![
                "qemu-system-x86",
                "qemu-system-arm",
                "mtools",
                "qemu-efi-aarch64",
            ],
        ))
    } else if std::process::Command::new("dnf")
        .arg("--version")
        .output()
        .is_ok()
    {
        Some((
            "dnf",
            vec![
                "qemu-system-x86-core",
                "qemu-system-aarch64",
                "mtools",
                "edk2-aarch64",
            ],
        ))
    } else if std::process::Command::new("pacman")
        .arg("--version")
        .output()
        .is_ok()
    {
        Some(("pacman", vec!["qemu", "mtools", "edk2-aarch64"]))
    } else {
        None
    }
}

fn install_linux() -> Result<(), String> {
    let Some((mgr, pkgs)) = linux_pkg_manager() else {
        return Err("could not detect a supported package manager on this Linux distro".into());
    };
    let deps = scan_native();
    let want_qemu = !(deps.qemu_x64 && deps.qemu_arm64);
    let want_mtools = !deps.mtools;
    let want_fw = deps.firmware.is_none();
    if !(want_qemu || want_mtools || want_fw) {
        return Ok(());
    }

    match mgr {
        "apt" => {
            crate::steps::note("updating apt index (may prompt for sudo) ...");
            checked("sudo", &["apt-get", "update"], "apt-get update")?;
            let mut args = vec!["apt-get", "install", "-y"];
            args.extend(pkgs.iter().copied());
            checked("sudo", &args, "apt-get install native deps")?;
        }
        "dnf" => {
            let mut args = vec!["-y"];
            args.extend(pkgs.iter().copied());
            let cmd = if have("sudo") { "sudo" } else { "dnf" };
            let mut full = vec!["dnf".to_string()];
            full.extend(args.iter().map(|s| s.to_string()));
            checked(
                cmd,
                &full.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "dnf install native deps",
            )?;
        }
        "pacman" => {
            let mut args = vec!["-S", "--needed", "--noconfirm"];
            args.extend(pkgs.iter().copied());
            let cmd = if have("sudo") { "sudo" } else { "pacman" };
            let mut full = vec!["pacman".to_string()];
            full.extend(args.iter().map(|s| s.to_string()));
            checked(
                cmd,
                &full.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "pacman install native deps",
            )?;
        }
        _ => return Err(format!("unsupported package manager '{mgr}'")),
    }
    Ok(())
}
