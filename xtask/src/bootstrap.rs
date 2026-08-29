use crate::platform;
use crate::rustup;
use crate::steps;
use clap::Args;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const LIMINE_VERSION: &str = "v12.6.0";
const LIMINE_URL: &str =
    "https://github.com/Limine-Bootloader/Limine/releases/download/v12.6.0/limine-binary.zip";
const LIMINE_SHA256: &str = "cbbc0a68da766faf05c14fdde31710563c5e6a89b6f2b012a57540d0cfdce822";

/// Files resolved by name anywhere in the archive (upstream reshuffles safe).
/// Platform-independent payloads staged into the image; the host-side `limine`
/// installer binary is resolved separately at image time (limine.exe on
/// Windows, a `limine` binary on PATH on Linux/macOS).
const REQUIRED_FILES: &[&str] = &[
    "limine-bios.sys",
    "limine-bios-cd.bin",
    "limine-uefi-cd.bin",
    "BOOTX64.EFI",
    "BOOTIA32.EFI",
    "BOOTAA64.EFI",
];

#[derive(Args)]
pub struct BootstrapArgs {
    /// Skip the rustup toolchain/component step.
    #[arg(long)]
    no_toolchain: bool,
    /// Skip installing native OS packages (qemu, mtools).
    #[arg(long)]
    no_native: bool,
}

pub fn run(repo_root: &Path, args: BootstrapArgs) -> Result<(), String> {
    if !args.no_toolchain {
        steps::step("Rust toolchain");
        rustup::ensure_toolchain(repo_root)?;
    }

    steps::step("Native OS packages (qemu, mtools, edk2 firmware)");
    if args.no_native {
        steps::note("skipping native package install (--no-native)");
        scan_report();
    } else {
        platform::install_native()?;
        scan_report();
    }

    steps::step("Limine bootloader");
    ensure_limine(repo_root)?;

    steps::step("UEFI firmware (aarch64)");
    stage_firmware(repo_root)?;

    println!("\nBootstrap complete: environment ready.");
    Ok(())
}

fn scan_report() {
    let deps = platform::scan_native();
    for m in platform::missing(&deps) {
        steps::fail(&format!("still missing: {m}"));
    }
    if let Some(fw) = &deps.firmware {
        steps::ok(&format!("edk2 firmware available at {}", fw.display()));
    }
}

fn ensure_limine(repo_root: &Path) -> Result<(), String> {
    let limine_dir = repo_root.join("third_party").join("limine");
    let version_marker = limine_dir.join("LIMINE_VERSION");
    let marker_matches = std::fs::read_to_string(&version_marker)
        .map(|s| s.trim() == LIMINE_VERSION)
        .unwrap_or(false);
    let have_all = marker_matches && REQUIRED_FILES.iter().all(|f| limine_dir.join(f).is_file());

    if have_all {
        steps::ok(&format!(
            "limine {LIMINE_VERSION} already present in third_party/limine/"
        ));
        return Ok(());
    }

    std::fs::create_dir_all(&limine_dir)
        .map_err(|e| format!("cannot create {limine_dir:?}: {e}"))?;

    let work = std::env::temp_dir();
    let tmp_zip = work.join(format!("ferric-k-limine-{LIMINE_VERSION}.zip"));
    let extract_dir = work.join(format!("ferric-k-limine-{LIMINE_VERSION}-extract"));

    steps::note(&format!("downloading {LIMINE_URL}"));
    download(LIMINE_URL, &tmp_zip)?;

    let actual = sha256_file(&tmp_zip)?;
    if actual != LIMINE_SHA256 {
        let _ = std::fs::remove_file(&tmp_zip);
        return Err(format!(
            "Checksum mismatch for limine-binary.zip!\n  expected: {LIMINE_SHA256}\n  actual:   {actual}\nRefusing to extract. If upstream re-published the asset, update the pin deliberately."
        ));
    }
    steps::ok("sha256 checksum matches pinned value");

    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir)
            .map_err(|e| format!("cannot clear {extract_dir:?}: {e}"))?;
    }
    extract_zip(&tmp_zip, &extract_dir)?;
    let _ = std::fs::remove_file(&tmp_zip);

    for file in REQUIRED_FILES {
        let matches: Vec<PathBuf> = walk(&extract_dir, file);
        if matches.len() != 1 {
            return Err(format!(
                "Expected exactly one '{file}' in the archive, found {}.",
                matches.len()
            ));
        }
        std::fs::copy(&matches[0], limine_dir.join(file))
            .map_err(|e| format!("cannot copy {file}: {e}"))?;
    }
    if cfg!(windows) {
        // Windows needs the host-side installer (limine.exe) for bios-install.
        let matches: Vec<PathBuf> = walk(&extract_dir, "limine.exe");
        if matches.len() != 1 {
            return Err(format!(
                "Expected exactly one 'limine.exe' in the archive (Windows host), found {}.",
                matches.len()
            ));
        }
        std::fs::copy(&matches[0], limine_dir.join("limine.exe"))
            .map_err(|e| format!("cannot copy limine.exe: {e}"))?;
    }
    let _ = std::fs::remove_dir_all(&extract_dir);

    std::fs::write(&version_marker, LIMINE_VERSION)
        .map_err(|e| format!("cannot write version marker: {e}"))?;
    steps::ok(&format!(
        "limine {LIMINE_VERSION} materialized into third_party/limine/"
    ));
    Ok(())
}

fn stage_firmware(repo_root: &Path) -> Result<(), String> {
    let src = platform::find_firmware_from_qemu()
        .or_else(platform::find_firmware_known)
        .ok_or_else(|| {
            "no edk2-aarch64 UEFI firmware found next to qemu-system-aarch64 (share/qemu)"
                .to_string()
        })?;

    let fw_dir = repo_root.join("third_party").join("firmware");
    let dst = fw_dir.join("edk2-aarch64-code.fd");
    if dst.is_file() && file_sha256(&dst)? == file_sha256(&src)? {
        steps::ok("third_party/firmware/edk2-aarch64-code.fd matches installed QEMU");
        return Ok(());
    }
    std::fs::create_dir_all(&fw_dir).map_err(|e| format!("cannot create {fw_dir:?}: {e}"))?;
    std::fs::copy(&src, &dst).map_err(|e| format!("cannot stage firmware: {e}"))?;
    steps::ok(&format!(
        "staged {} -> third_party/firmware/",
        src.display()
    ));
    Ok(())
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let body = ureq::get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut reader = body.into_reader();
    let mut f = std::fs::File::create(dest).map_err(|e| format!("cannot create {dest:?}: {e}"))?;
    std::io::copy(&mut reader, &mut f).map_err(|e| format!("write failed: {e}"))?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open {path:?}: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    sha256_file(path)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip).map_err(|e| format!("cannot open {zip:?}: {e}"))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("invalid zip: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("zip entry: {e}"))?;
        // Sanitize: only accept relative, nested-safe paths.
        let name = entry.name().to_string();
        if entry.is_dir() {
            std::fs::create_dir_all(dest.join(&name)).map_err(|e| format!("mkdir: {e}"))?;
            continue;
        }
        if name.contains("..") {
            continue;
        }
        let out = dest.join(&name);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let mut out_f = std::fs::File::create(&out).map_err(|e| format!("create {out:?}: {e}"))?;
        std::io::copy(&mut entry, &mut out_f).map_err(|e| format!("extract write: {e}"))?;
    }
    Ok(())
}

fn walk(dir: &Path, filename: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p, filename));
            } else if p.file_name().map(|n| n == filename).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}
