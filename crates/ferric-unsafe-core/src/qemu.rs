//! QEMU proof-of-execution helper (`isa-debug-exit`).
//!
//! Until the first output device exists (16550 UART), the only
//! way to *prove* the kernel booted through Limine and finished early init is
//! a side channel: QEMU's `isa-debug-exit` ISA device.
//!
//! Device contract, transcribed from QEMU `hw/misc/debugexit.c` (upstream
//! master):
//! - properties default to `iobase = 0x501`, `iosize = 0x02`;
//! - any write into `[iobase, iobase + iosize)` makes QEMU shut down with
//!   exit code `(value << 1) | 1`.
//!
//! `scripts/run.ps1` attaches the device at exactly [`DEBUG_EXIT_IO_BASE`]
//! and asserts the exit codes produced by [`STATUS_BOOT_OK`] /
//! [`STATUS_BOOT_INFO_MISSING`] through [`qemu_exit_code`]. The constants are
//! mirrored between the two files on purpose — keep them in sync (see the
//! decision log in ARCHITECTURE.md).

/// `iobase` the harness configures for `-device isa-debug-exit`.
///
/// QEMU's own default (`hw/misc/debugexit.c`,
/// `DEFINE_PROP_UINT32("iobase", ..., 0x501)`); restated explicitly so both
/// sides of the contract name one source of truth.
pub const DEBUG_EXIT_IO_BASE: u16 = 0x501;

/// Status byte written after entry + early init completed successfully
/// (Limine handoff accepted and all mandatory boot-info responses present).
///
/// Produces QEMU exit code 33 through [`qemu_exit_code`].
pub const STATUS_BOOT_OK: u8 = 0x10;

/// Status byte written when the kernel ran but mandatory boot info was
/// rejected or unanswered ([`crate::limine::collect`] returned `None`).
///
/// Produces QEMU exit code 65, letting the smoke harness fail fast with a
/// distinct diagnostic instead of waiting for the timeout.
pub const STATUS_BOOT_INFO_MISSING: u8 = 0x20;

/// The exit code QEMU reports for a given status byte:
/// `(value << 1) | 1` (QEMU `debug_exit_write`).
///
/// Always odd, so legitimate QEMU crash/reset exits (small even numbers, 1)
/// can never collide with a kernel-reported status.
#[must_use]
pub const fn qemu_exit_code(status: u8) -> i32 {
    ((status as i32) << 1) | 1
}

/// Writes `status` to the isa-debug-exit port; QEMU then exits the emulator
/// process with [`qemu_exit_code(status)`].
///
/// This is the intended end of the bring-up path while the kernel has no
/// console: both smoke runs and manual `run.ps1` invocations terminate QEMU
/// deterministically once early init finishes.
#[cfg(all(target_arch = "x86_64", not(test)))]
pub fn debug_exit(status: u8) -> ! {
    // SAFETY: one byte out to the isa-debug-exit I/O port whose base the
    // run harness pinned at `DEBUG_EXIT_IO_BASE`. On QEMU's q35 machine no
    // other device decodes 0x501, and the emulator terminates on this write
    // instead of returning, so no further state is observable afterwards.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") DEBUG_EXIT_IO_BASE,
            in("al") status,
            options(nostack, nomem, preserves_flags)
        );
    }
    unreachable!("isa-debug-exit writes terminate QEMU before control returns");
}

// ---------------------------------------------------------------------------
// Host-side tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_follows_qemu_debugexit_formula() {
        assert_eq!(qemu_exit_code(STATUS_BOOT_OK), 33);
        assert_eq!(qemu_exit_code(STATUS_BOOT_INFO_MISSING), 65);
        assert_eq!(qemu_exit_code(0x00), 1);
        assert_eq!(qemu_exit_code(0xFF), 511);
    }

    #[test]
    fn status_codes_map_to_odd_exit_codes_that_cannot_collide() {
        // Oddness is the collision guard against non-kernel exit paths
        // (crashes, resets, harness failures report even codes or 1).
        for status in [STATUS_BOOT_OK, STATUS_BOOT_INFO_MISSING] {
            assert_ne!(qemu_exit_code(status) % 2, 0);
        }
        assert_ne!(
            qemu_exit_code(STATUS_BOOT_OK),
            qemu_exit_code(STATUS_BOOT_INFO_MISSING)
        );
    }

    #[test]
    fn documented_io_base_matches_qemu_default() {
        // hw/misc/debugexit.c: DEFINE_PROP_UINT32("iobase", ..., 0x501).
        assert_eq!(DEBUG_EXIT_IO_BASE, 0x501);
    }
}
