use clap::{Parser, Subcommand};

mod bootstrap;
mod build;
mod check;
mod clean;
mod elf;
mod exception_demo;
mod image;
mod panic_demo;
mod platform;
mod qmp;
mod runner;
mod rustup;
mod steps;
mod util;

use bootstrap::BootstrapArgs;
use build::BuildArgs;
use check::CheckArgs;
use clean::CleanArgs;
use exception_demo::ExceptionDemoArgs;
use image::ImageArgs;
use panic_demo::PanicDemoArgs;
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
    /// Remove the build cache (target/ and build/).
    Clean(CleanArgs),
    /// Rebuild both kernel ELFs (x86_64 + aarch64).
    Build(BuildArgs),
    /// Assemble the dual-arch bootable disk image.
    BuildImage(ImageArgs),
    /// Boot the image under QEMU (interactive, or --smoke assertions).
    Run(RunArgs),
    /// Build panic-enabled kernels, boot both arches, assert the crash panel.
    PanicDemo(PanicDemoArgs),
    /// Build exception-enabled kernels, boot both arches, assert the diagnostic dump.
    ExceptionDemo(ExceptionDemoArgs),
    /// Full quality gate: fmt, clippy, build, ELF/Limine checks, tests, smoke boots.
    Check(CheckArgs),
}

fn main() {
    let cli = Cli::parse();
    let repo_root = util::repo_root();
    let result = match cli.command {
        Command::Bootstrap(args) => bootstrap::run(&repo_root, args),
        Command::Clean(args) => clean::run(&repo_root, args),
        Command::Build(args) => build::run(&repo_root, args),
        Command::BuildImage(args) => image::run(&repo_root, args),
        Command::Run(args) => runner::run(&repo_root, args),
        Command::PanicDemo(args) => panic_demo::run(&repo_root, args),
        Command::ExceptionDemo(args) => exception_demo::run(&repo_root, args),
        Command::Check(args) => check::run(&repo_root, args),
    };
    if let Err(e) = result {
        eprintln!("\n{}", e);
        std::process::exit(1);
    }
}
