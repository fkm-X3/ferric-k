use clap::{Parser, Subcommand};

mod bootstrap;
mod check;
mod elf;
mod image;
mod platform;
mod runner;
mod rustup;
mod steps;
mod util;

use bootstrap::BootstrapArgs;
use check::CheckArgs;
use image::ImageArgs;
use runner::RunArgs;

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Ferric-K cross-platform build/check/run harness (replaces scripts/*.ps1)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the pinned Rust toolchain + native build deps (qemu, mtools, limine).
    Bootstrap(BootstrapArgs),
    /// Assemble the dual-arch bootable disk image.
    BuildImage(ImageArgs),
    /// Boot the image under QEMU (interactive, or --smoke assertions).
    Run(RunArgs),
    /// Full quality gate: fmt, clippy, build, ELF/Limine checks, tests, smoke boots.
    Check(CheckArgs),
}

fn main() {
    let cli = Cli::parse();
    let repo_root = util::repo_root();
    let result = match cli.command {
        Command::Bootstrap(args) => bootstrap::run(&repo_root, args),
        Command::BuildImage(args) => image::run(&repo_root, args),
        Command::Run(args) => runner::run(&repo_root, args),
        Command::Check(args) => check::run(&repo_root, args),
    };
    if let Err(e) = result {
        eprintln!("\n{}", e);
        std::process::exit(1);
    }
}
