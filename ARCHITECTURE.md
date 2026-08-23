# Ferric-K Architecture

This document states the boundary rules every change must obey, followed by the
decision log (append-only). When code and this document disagree, one of them is
wrong and must be fixed before the next commit.

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
- `.cargo/config.toml` enables `build-std = ["core", "compiler_builtins"]`
  globally plus `json-target-spec` (required for JSON specs on current
  nightlies).

## Quality gates

`scripts/check.ps1` must pass before any commit:
fmt → clippy (both targets, `-D warnings`) → build both targets → ELF sanity.
Warnings-as-errors is enforced at the clippy stage rather than via
`RUSTFLAGS=-Dwarnings` on raw builds, because `-D warnings` applied to
`build-std` compilations of *upstream* sources would break builds on unrelated
upstream warnings.

Host-side tests run without `--target` and get their
own clippy pass added to the gate then.
