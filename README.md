# Ferric-K

The v2 of Alloy-OS

Ferric-K is a lot more secure compared to Alloy-OS

## Commands

The build/check/run harness is a cross-platform Rust xtask (in `xtask/`); it
auto-detects the host OS and installs native deps accordingly (MSYS2 pacman on
Windows, Homebrew on macOS, apt/dnf/pacman on Linux).

```text
cargo xtask bootstrap      Install the pinned Rust toolchain + native build deps (qemu, mtools, edk2 firmware, Limine)
cargo xtask build-image    Assemble the dual-arch bootable disk image (--image-path, --size-mb)
cargo xtask run            Boot the image under QEMU (--arch x64|arm64, --smoke)
cargo xtask check          Full quality gate: fmt, clippy, build, ELF/Limine checks, tests, smoke boots (--no-smoke)
```
