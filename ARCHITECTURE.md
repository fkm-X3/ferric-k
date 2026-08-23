# Ferric-K Architecture

This document states the boundary rules every change must obey. When code and this document disagree, one of them is wrong and must be fixed before the next commit.

## Crate layering

```
                 ┌────────────────┐
                 │  ferric-kernel │  bin, #![forbid(unsafe_code)]
                 └───┬────┬────┬──┘
        ┌────────────┘    │    └────────────┐
        ▼                 ▼                 ▼
┌───────────────┐ ┌───────────────┐ ┌────────────────────┐
│ ferric-api    │ │ ferric-safe-  │ │ ferric-unsafe-core │
│ traits/types  │ │ core          │ │ ALL unsafe lives   │
│ #![forbid]    │ │ console/font/ │ │ here: entry, UART, │
│               │ │ log/shell     │ │ fb, locks, Limine  │
└───────────────┘ └──────┬────────┘ │ #![no forbid]      │
      ▲                  │          └─────────┬──────────┘
      │                  ▼                    │
      └──────────────────┴────────────────────┘
              ferric-api is the contract;
              unsafe-core implements it per arch
```

### Dependency rules

1. `ferric-api` depends on nothing. It defines arch-neutral traits
   (`TextSink`, `TimeSource`, `MachineInfo`, ...). Safe crates program against
   these traits; `ferric-unsafe-core` implements them per architecture.
2. `ferric-safe-core` may depend on `ferric-api` only. No hardware access,
   ever — it is pure logic (byte buffers, trait objects) so it can be tested on
   the host.
3. `ferric-unsafe-core` may depend on `ferric-api`. It is the **only** crate
   allowed to contain `unsafe`, and its **public API must be 100% safe**:
   hardware access is wrapped in types that make misuse impossible
   (bounds-checked framebuffer, initialized-once globals, lock guards).
4. `ferric-kernel` may depend on all three. It is a thin wiring layer: bring-up
   calls into safe `kmain`. It carries `#![forbid(unsafe_code)]`.

### Unsafe rules (compiler-enforced)

- Every `unsafe` block carries a `// SAFETY:` justification
  (`clippy::undocumented_unsafe_blocks = deny`).
- `deny(unsafe_op_in_unsafe_fn)` workspace-wide: unsafety inside an `unsafe fn`
  must still be explicitly scoped and justified.
- All other crates carry `#![forbid(unsafe_code)]`. Note that this also forbids
  ABI-affecting attributes (`#[no_mangle]`, `unsafe extern` blocks) — symbols
  like `_start` therefore live in `ferric-unsafe-core`.

## Targets & toolchain

- Toolchain pinned in `rust-toolchain.toml`.
- Custom target specs in `targets/*.json`, restored from git history and
  audited:
  - **x86_64**: `code-model: kernel`, `disable-redzone: true`, `panic-strategy: abort`
    (no unwinding in kernels), `relocation-model: static` (no dynamic loader),
    `-mmx,-sse,+sse2` (SSE save/restore never initialized by us; SSE2 kept as
    baseline for compiler intrinsics).
  - **aarch64**: `+strict-align` (do not assume unaligned access is permitted
    at EL1), `+neon` (explicit SIMD register availability for LLVM), same
    panic/relocation policy as x86_64.
- `-Zbuild-std = ["core", "compiler_builtins"]` and `-Zjson-target-spec` are
  required to build for these specs on the pinned nightly. They are passed
  **explicitly per kernel-target invocation** in `scripts/check.ps1` — never as
  a global `.cargo/config.toml` `[unstable]` table, which would also apply to
  host builds and rebuild `core` behind std (duplicate-lang-item breakage).

## Quality gates

`scripts/check.ps1` must pass before any commit:

fmt → clippy (host: libs + tests) → clippy (both custom targets,
`-D warnings`) → build both targets (+ ELF sanity) → Limine structural gate
(x86_64 image: higher-half entry window, PT_LOAD alignment/base, byte-exact
`.limine_requests` contents) → host unit tests.

Warnings-as-errors is enforced at the clippy stages rather than via
`RUSTFLAGS=-Dwarnings` on raw builds, because `-D warnings` applied to
`build-std` compilations of *upstream* sources would break builds on unrelated
upstream warnings. The freestanding kernel bin is never built for the host; its
panic handler is `#[cfg(not(test))]` so its implicit test target can use std's.
