use crate::platform;
use crate::steps;
use crate::util;
use clap::Args;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

const SECTOR_SIZE: u64 = 512;
const HEADS: u32 = 64;
const SECTORS_PER_TRACK: u32 = 32;
const FIRST_PART_START_SECTOR: u32 = 2048;
const FAT_TYPE_WITH_LBA: u8 = 0x0E;

#[derive(Args)]
pub struct ImageArgs {
    /// Output image path (default build/ferric.img).
    #[arg(long, default_value = "build/ferric.img")]
    pub image_path: String,
    /// Image size in MiB.
    #[arg(long, default_value_t = 64)]
    pub size_mb: u32,
}

pub fn run(repo_root: &Path, args: ImageArgs) -> Result<(), String> {
    for tool in platform::MTOOLS {
        util::find(tool).map_err(|e| format!("{e} (run: cargo xtask bootstrap)"))?;
    }

    let limine_dir = repo_root.join("third_party").join("limine");
    if !limine_dir.join("limine-bios.sys").is_file() {
        return Err("third_party/limine missing. Run: cargo xtask bootstrap".into());
    }
    // Host-side image installer: Windows uses the third_party limine.exe;
    // Linux/macOS use a `limine` binary on PATH (distro package or built
    // from source).
    let limine_exe = if cfg!(windows) {
        let p = limine_dir.join("limine.exe");
        p.is_file().then_some(p).ok_or_else(|| {
            "third_party/limine/limine.exe missing. Run: cargo xtask bootstrap".to_string()
        })?
    } else {
        util::find("limine")
            .map_err(|_| "no `limine` binary on PATH. Install the distro's limine package (or build from source), then rerun".to_string())?
    };

    let kernels: &[(&str, &str)] = &[
        (
            "kernel-x86_64.elf",
            "target/x86_64-ferric/debug/ferric-kernel",
        ),
        (
            "kernel-aarch64.elf",
            "target/aarch64-ferric/debug/ferric-kernel",
        ),
    ];
    for (name, rel) in kernels {
        let p = repo_root.join(rel);
        let target = target_for(name);
        if !p.is_file() {
            return Err(format!(
                "{name} not found at {}. Build it first: cargo build --target targets/{target}.json -Zbuild-std=core,compiler_builtins -Zjson-target-spec",
                p.display()
            ));
        }
    }

    let conf = repo_root.join("boot/limine.conf");
    let bios_sys = limine_dir.join("limine-bios.sys");
    let uefi = [
        limine_dir.join("BOOTX64.EFI"),
        limine_dir.join("BOOTIA32.EFI"),
        limine_dir.join("BOOTAA64.EFI"),
    ];
    for f in std::iter::once(&conf)
        .chain(std::iter::once(&bios_sys))
        .chain(uefi.iter())
    {
        if !f.is_file() {
            return Err(format!("missing input: {}", f.display()));
        }
    }

    steps::step("Create image");
    let image_path = repo_root.join(&args.image_path);
    if let Some(parent) = image_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }

    let total_sectors = (args.size_mb as u64) * 1024 * 1024 / SECTOR_SIZE;
    let part_sectors = total_sectors - FIRST_PART_START_SECTOR as u64;

    {
        // Scoped so the write handle is closed before external tools
        // (mformat/mcopy/limine.exe) touch the image; an open handle can lock
        // the file on Windows and break the later boot-sector read.
        let mut img =
            std::fs::File::create(&image_path).map_err(|e| format!("cannot create image: {e}"))?;
        img.set_len(total_sectors * SECTOR_SIZE)
            .map_err(|e| format!("cannot size image: {e}"))?;

        // MBR partition table entry (LBA fields are authoritative; CHS cosmetic).
        let mut mbr = [0u8; 512];
        mbr[446] = 0x80;
        let chs = chs_bytes(
            FIRST_PART_START_SECTOR,
            total_sectors as u32 - FIRST_PART_START_SECTOR,
        );
        mbr[447] = chs.start[0];
        mbr[448] = chs.start[1];
        mbr[449] = chs.start[2];
        mbr[450] = FAT_TYPE_WITH_LBA;
        mbr[451] = chs.end[0];
        mbr[452] = chs.end[1];
        mbr[453] = chs.end[2];
        mbr[454..458].copy_from_slice(&FIRST_PART_START_SECTOR.to_le_bytes());
        mbr[458..462].copy_from_slice(&(part_sectors as u32).to_le_bytes());
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        img.seek(SeekFrom::Start(0)).unwrap();
        img.write_all(&mbr)
            .map_err(|e| format!("cannot write MBR: {e}"))?;
    }
    steps::ok(&format!(
        "{}: {} MiB, partition 1 type 0x{FAT_TYPE_WITH_LBA:02X} @ LBA {}",
        args.image_path, args.size_mb, FIRST_PART_START_SECTOR
    ));

    let offset = FIRST_PART_START_SECTOR as u64 * SECTOR_SIZE;
    let img_at_off = format!("{}@@{}", image_path.display(), offset);

    steps::step("Format FAT");
    let tracks = (part_sectors / (HEADS * SECTORS_PER_TRACK) as u64) as u32;
    run_tool(
        "mformat",
        &[
            "-i",
            &img_at_off,
            "-t",
            &tracks.to_string(),
            "-h",
            &HEADS.to_string(),
            "-s",
            &SECTORS_PER_TRACK.to_string(),
            "::",
        ],
        "mformat FAT16",
    )?;

    steps::step("Stage files");
    run_tool(
        "mmd",
        &["-i", &img_at_off, "::EFI", "::EFI/BOOT"],
        "mmd EFI/BOOT",
    )?;
    run_tool(
        "mcopy",
        &[
            "-i",
            &img_at_off,
            &conf.display().to_string(),
            "::limine.conf",
        ],
        "copy limine.conf",
    )?;
    for (name, rel) in kernels {
        let p = repo_root.join(rel).display().to_string();
        run_tool(
            "mcopy",
            &["-i", &img_at_off, &p, &format!("::{name}")],
            &format!("copy {name}"),
        )?;
    }
    run_tool(
        "mcopy",
        &[
            "-i",
            &img_at_off,
            &bios_sys.display().to_string(),
            "::limine-bios.sys",
        ],
        "copy limine-bios.sys",
    )?;
    run_tool(
        "mcopy",
        &[
            "-i",
            &img_at_off,
            &uefi[0].display().to_string(),
            &uefi[1].display().to_string(),
            &uefi[2].display().to_string(),
            "::EFI/BOOT/",
        ],
        "copy UEFI loaders",
    )?;

    steps::step("Install Limine BIOS stages");
    let status = std::process::Command::new(&limine_exe)
        .arg("bios-install")
        .arg(&image_path)
        .status()
        .map_err(|e| format!("failed to run limine bios-install: {e}"))?;
    if !status.success() {
        return Err("limine bios-install failed".into());
    }
    steps::ok("limine bios-install");

    steps::step("Validate image");
    let listing = run_capture("mdir", &["-i", &img_at_off, "::"])?;
    let boot = run_capture("mdir", &["-i", &img_at_off, "::EFI/BOOT"])?;
    let flat = (listing + &boot).to_lowercase();
    for needle in [
        "limine.conf",
        "kernel-x86_64",
        "kernel-aarch64",
        "limine-bios.sys",
        "bootx64",
        "bootia32",
        "bootaa64",
    ] {
        if !flat.contains(needle) {
            return Err(format!(
                "image validation: '{needle}' not found in FAT directory listing"
            ));
        }
    }
    steps::ok("FAT contents complete");

    let mut boot_sector = [0u8; 512];
    {
        use std::io::Read;
        let mut f =
            std::fs::File::open(&image_path).map_err(|e| format!("cannot open image: {e}"))?;
        f.read_exact(&mut boot_sector)
            .map_err(|e| format!("cannot read boot sector: {e}"))?;
    }
    if boot_sector[0..8].iter().all(|&b| b == 0) {
        return Err(
            "image validation: MBR boot code area is still empty (bios-install no-op?)".into(),
        );
    }
    steps::ok("MBR contains installed Limine stages");

    println!("\nImage ready: {}", image_path.display());
    Ok(())
}

struct ChsPair {
    start: [u8; 3],
    end: [u8; 3],
}

fn chs_bytes(start_sector: u32, part_sectors: u32) -> ChsPair {
    ChsPair {
        start: Chs::from_lba(start_sector).b,
        end: Chs::from_lba(start_sector + part_sectors - 1).b,
    }
}

struct Chs {
    b: [u8; 3],
}

impl Chs {
    fn from_lba(lba: u32) -> Chs {
        let head = ((lba / SECTORS_PER_TRACK) % HEADS).min(255) as u8;
        let cyl = (lba / (SECTORS_PER_TRACK * HEADS)).min(1023) as u16;
        let sect = ((lba % SECTORS_PER_TRACK) + 1).min(63) as u8;
        Chs {
            b: [
                head,
                ((sect & 0x3F) | (((cyl >> 8) & 0x3) as u8) << 6),
                (cyl & 0xFF) as u8,
            ],
        }
    }
}

fn run_tool(program: &str, args: &[&str], what: &str) -> Result<(), String> {
    util::checked(program, args, what)
}

fn run_capture(program: &str, args: &[&str]) -> Result<String, String> {
    let out = util::run(program, args).map_err(|e| format!("failed to run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} failed (exit {:?})", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn target_for(name: &str) -> &'static str {
    if name.contains("x86_64") {
        "x86_64-ferric"
    } else {
        "aarch64-ferric"
    }
}
