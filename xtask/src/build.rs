use crate::elf;
use crate::steps;
use crate::util;
use clap::Args;
use std::path::Path;

/// Kernel-target builds need these flags explicitly per-invocation (a global
/// [unstable] build-std would poison host builds). Matches check.rs.
const KERNEL_CARGO_ARGS: [&str; 2] = [
    "-Zbuild-std=core,alloc,compiler_builtins",
    "-Zjson-target-spec",
];

const TARGETS: [(&str, &str, u16, &str); 2] = [
    // (target, spec, e_machine, name)
    ("x86_64-ferric", "targets/x86_64-ferric.json", 0x3E, "EM_X86_64"),
    ("aarch64-ferric", "targets/aarch64-ferric.json", 0xB7, "EM_AARCH64"),
];

#[derive(Args)]
pub struct BuildArgs {
    /// Skip ELF sanity + Limine gates after building (build only).
    #[arg(long)]
    no_checks: bool,
}

pub fn run(repo_root: &Path, args: BuildArgs) -> Result<(), String> {
    steps::step("rebuild both kernel ELFs");
    for &(target, spec, machine, machine_name) in TARGETS.iter() {
        steps::note(&format!("building {target} ..."));
        let mut cargo_args = vec!["build", "--target", spec];
        cargo_args.extend(KERNEL_CARGO_ARGS);
        util::checked("cargo", &cargo_args, &format!("build ({target})"))?;

        let elf_path = repo_root.join(format!("target/{target}/debug/ferric-kernel"));
        if args.no_checks {
            continue;
        }
        let elf = std::fs::read(&elf_path)
            .map_err(|e| format!("kernel ELF not found at {}: {e}", elf_path.display()))?;
        check_machine(&elf_path, &elf, machine, machine_name)?;
        elf::limine_elf_gate(&elf_path, &elf)?;
    }
    println!("\nBoth kernel ELFs rebuilt.");
    Ok(())
}

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn check_machine(path: &Path, elf: &[u8], want_machine: u16, want_name: &str) -> Result<(), String> {
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
    steps::ok(&format!("ELF valid: machine={want_name}"));
    Ok(())
}
