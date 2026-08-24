//! QEMU `isa-debug-exit` side channel — deterministic proof of completed
//! early init while no console exists. Device contract transcribed from QEMU
//! `hw/misc/debugexit.c`: any write to `[iobase, iobase + iosize)` shuts QEMU
//! down with exit code `(value << 1) | 1`. `scripts/run.ps1` attaches the
//! device at [`DEBUG_EXIT_IO_BASE`] and asserts the codes below; constants
//! are mirrored between the two files on purpose.

/// `iobase` for `-device isa-debug-exit` (QEMU default from `debugexit.c`).
pub const DEBUG_EXIT_IO_BASE: u16 = 0x501;

/// Early init completed successfully; QEMU exits with code 33.
pub const STATUS_BOOT_OK: u8 = 0x10;

/// Mandatory boot info rejected or unanswered; QEMU exits with code 65.
pub const STATUS_BOOT_INFO_MISSING: u8 = 0x20;

/// Exit code QEMU reports for a status byte: `(status << 1) | 1`. Always
/// odd, so crash/reset exits can never collide with a kernel-reported status.
#[must_use]
pub const fn qemu_exit_code(status: u8) -> i32 {
    ((status as i32) << 1) | 1
}

/// Writes `status` to the isa-debug-exit port; QEMU then terminates the
/// emulator process with [`qemu_exit_code(status)`].
#[cfg(all(target_arch = "x86_64", not(test)))]
pub fn debug_exit(status: u8) -> ! {
    // SAFETY: one byte out to the isa-debug-exit port whose base the run
    // harness pinned at `DEBUG_EXIT_IO_BASE`; on q35 no other device decodes
    // 0x501, and the emulator terminates on this write instead of returning.
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
        assert_eq!(DEBUG_EXIT_IO_BASE, 0x501);
    }
}
