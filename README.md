# Ferric-K

A bare-metal Rust kernel with a compiler-enforced safe/unsafe split, targeting
x86_64 and aarch64 from day one, booted by Limine, rendering text to a linear
framebuffer.

## Quick start

```sh
cargo xtask bootstrap   # install toolchain + native deps + Limine
cargo xtask check       # full quality gate (fmt, clippy, build, tests, smoke)
cargo xtask run --arch x64    # boot interactively in QEMU (x86_64)
cargo xtask run --arch arm64  # boot interactively in QEMU (aarch64)
```

## Commands

All commands are cross-platform via `cargo xtask` (auto-detects host OS;
installs native deps via MSYS2 pacman on Windows, Homebrew on macOS,
apt/dnf/pacman on Linux).

| Command | Description |
|---|---|
| `cargo xtask bootstrap` | Install the pinned Rust nightly + components, native deps (qemu, mtools, edk2 firmware), and checksum-pinned Limine into `third_party/`. |
| `cargo xtask build-image` | Assemble the dual-arch bootable FAT16 disk image (`--image-path`, `--size-mb`). |
| `cargo xtask run` | Boot under QEMU (`--arch x64\|arm64`, `--smoke`, `--image-path`). |
| `cargo xtask panic-demo` | Build with `panic-on-boot`, boot both arches, assert the red crash panel. |
| `cargo xtask check` | Full quality gate: fmt → clippy (host + both targets) → build + ELF/Limine gates → host tests (safe-core + unsafe-core) → QEMU smoke boots (x86_64 + aarch64). `--no-smoke` skips the QEMU steps. |

## Architecture

```
ferric-kernel   thin bin, #![forbid(unsafe_code)]
    ↓
ferric-api      arch-neutral traits (TextSink, ...)
ferric-safe-core   pure logic: console model, font, logging (host-testable)
ferric-unsafe-core   ALL unsafe lives here: entry, drivers, locks, Limine ABI
```

`ferric-unsafe-core` is the **only** crate permitted to contain `unsafe`; its
public API is 100% safe. Safe crates carry `#![forbid(unsafe_code)]` and are
tested on the host. See `ARCHITECTURE.md` for the full boundary rules and
decision log.

## Testing

- **Host tests**: `cargo test -p ferric-safe-core --lib` and
  `cargo test -p ferric-unsafe-core --lib` cover the pure-logic modules
  (font parsing, text grid, log filtering, sync primitives, driver register
  semantics against mock MMIO, Limine ABI layout, MMU descriptors).
- **Smoke boots**: `cargo xtask check` builds the dual-arch image and boots
  both kernels headless in QEMU, asserting serial banner markers
  (`BOOT OK`, `FRAMEBUFFER OK`, `Hello from Ferric-K!`) and the expected
  exit code per architecture.
- **Panic demo**: `cargo xtask panic-demo` exercises the red-screen crash
  panel path on both architectures.
