use crate::elf;
use crate::image;
use crate::runner;
use crate::steps;
use crate::util;
use clap::Args;
use std::path::Path;

/// Kernel-target builds need these flags explicitly per-invocation (a global
/// [unstable] build-std would poison host builds).
const KERNEL_CARGO_ARGS: [&str; 2] = ["-Zbuild-std=core,compiler_builtins", "-Zjson-target-spec"];

const EXPECTED_MACHINE: [(&str, u16, &str); 2] = [
    ("x86_64-ferric", 0x3E, "EM_X86_64"),
    ("aarch64-ferric", 0xB7, "EM_AARCH64"),
];

#[derive(Args)]
pub struct CheckArgs {
    /// Skip the QEMU smoke-boot steps (for hosts without QEMU).
    #[arg(long)]
    no_smoke: bool,
}

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn r64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

fn assert_ok(out: std::io::Result<std::process::Output>, what: &str) -> Result<(), String> {
    let out = out.map_err(|e| format!("failed to run: {e}"))?;
    if !out.status.success() {
        return Err(format!("CHECK FAILED during: {what}"));
    }
    steps::ok(what);
    Ok(())
}

const CARGO: &str = "cargo";

pub fn run(repo_root: &Path, args: CheckArgs) -> Result<(), String> {
    step_reset();

    steps::step("cargo fmt --check");
    assert_ok(util::run(CARGO, &["fmt", "--all", "--check"]), "formatting")?;

    // The freestanding bin cannot be checked on the host; target clippy covers it.
    steps::step("clippy (host: libs + tests)");
    assert_ok(
        util::run(
            CARGO,
            &[
                "clippy",
                "--workspace",
                "--lib",
                "--tests",
                "--",
                "-D",
                "warnings",
            ],
        ),
        "clippy (host)",
    )?;

    for &(target, _, _) in EXPECTED_MACHINE.iter() {
        steps::step(&format!("clippy --target {target}"));
        let target_json = format!("targets/{target}.json");
        let mut args = vec!["clippy", "--workspace", "--target", target_json.as_str()];
        args.extend(KERNEL_CARGO_ARGS);
        args.extend(["--", "-D", "warnings"]);
        assert_ok(util::run(CARGO, &args), &format!("clippy ({target})"))?;
    }

    for &(target, machine, machine_name) in EXPECTED_MACHINE.iter() {
        steps::step(&format!("build --target {target} (+ELF checks)"));
        let target_json = format!("targets/{target}.json");
        let mut args = vec!["build", "--target", target_json.as_str()];
        args.extend(KERNEL_CARGO_ARGS);
        assert_ok(util::run(CARGO, &args), &format!("build ({target})"))?;

        let elf_path = repo_root.join(format!("target/{target}/debug/ferric-kernel"));
        let elf = std::fs::read(&elf_path)
            .map_err(|e| format!("kernel ELF not found at {}: {e}", elf_path.display()))?;
        check_elf_basic(&elf_path, &elf, machine, machine_name)?;
        elf::limine_elf_gate(&elf_path, &elf)?;
    }

    steps::step("test (host: ferric-safe-core + ferric-unsafe-core)");
    assert_ok(
        util::run(CARGO, &["test", "-p", "ferric-safe-core", "--lib"]),
        "host tests (ferric-safe-core)",
    )?;
    assert_ok(
        util::run(CARGO, &["test", "-p", "ferric-unsafe-core", "--lib"]),
        "host tests (ferric-unsafe-core)",
    )?;

    if args.no_smoke {
        steps::note("skipping disk image + smoke boots (--no-smoke)");
    } else {
        steps::step("disk image + QEMU smoke boots");
        image::run(
            repo_root,
            image::ImageArgs {
                image_path: "build/ferric.img".into(),
                size_mb: 64,
            },
        )?;
        runner::run(
            repo_root,
            runner::RunArgs {
                arch: "x64".into(),
                smoke: true,
                image_path: Some("build/ferric.img".into()),
                smoke_timeout_sec: 120,
            },
        )?;
        runner::run(
            repo_root,
            runner::RunArgs {
                arch: "arm64".into(),
                smoke: true,
                image_path: Some("build/ferric.img".into()),
                smoke_timeout_sec: 120,
            },
        )?;
    }

    println!(
        "\nCHECK PASSED: fmt + clippy(host,x86_64,aarch64) + build(x2) + ELF/Limine gates + host tests(safe-core,unsafe-core) + smoke boots(x86_64,aarch64) all green."
    );
    Ok(())
}

fn check_elf_basic(
    path: &Path,
    elf: &[u8],
    want_machine: u16,
    want_name: &str,
) -> Result<(), String> {
    let p = path.display();
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" {
        return Err(format!("{p} is not an ELF file"));
    }
    let machine = r16(elf, 0x12);
    if machine != want_machine {
        return Err(format!(
            "{p}: e_machine 0x{machine:04X}, expected {want_name} (0x{want_machine:04X})"
        ));
    }
    let entry = r64(elf, 0x18);
    if entry == 0 {
        return Err(format!("{p}: entry point is zero (_start not linked)"));
    }
    steps::ok(&format!(
        "ELF valid: machine={want_name}, entry=0x{entry:X}"
    ));
    Ok(())
}

fn step_reset() {
    // steps::step uses a monotonic counter starting at 0; reset via re-init.
    crate::steps::reset_counter();
}
