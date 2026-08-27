//! QEMU exit channels for deterministic smoke-boot termination.
//!
//! x86_64: `isa-debug-exit`, contract transcribed from QEMU
//! `hw/misc/debugexit.c`: any write to `[iobase, iobase + iosize)` shuts QEMU
//! down with exit code `(value << 1) | 1`. `scripts/run.ps1` attaches the
//! device at [`DEBUG_EXIT_IO_BASE`] and asserts the serial banner plus the
//! exit code; constants are mirrored between the two files on purpose.
//!
//! aarch64: ARM semihosting `SYS_EXIT` (ARM DEN 0024A), which passes the
//! subcode through as the raw QEMU process exit code. Requires the emulator
//! to run with `-semihosting-config enable=on,target=native`, which
//! `scripts/run.ps1` always supplies.

/// `iobase` for `-device isa-debug-exit` (QEMU default from `debugexit.c`).
pub const DEBUG_EXIT_IO_BASE: u16 = 0x501;

/// Early init completed successfully; QEMU exits with code 33.
pub const STATUS_BOOT_OK: u8 = 0x10;

/// Mandatory boot info rejected or unanswered; QEMU exits with code 65.
pub const STATUS_BOOT_INFO_MISSING: u8 = 0x20;

/// UART loopback probe failed (port absent or dead); QEMU exits with code 97.
pub const STATUS_UART_FAULT: u8 = 0x30;

/// No usable RGB framebuffer was handed over; QEMU exits with code
/// `(0x40 << 1) | 1` on x86_64 and `0x40` raw on aarch64.
pub const STATUS_FRAMEBUFFER_MISSING: u8 = 0x40;

/// The color-bar self-test failed its readback; QEMU exits with code
/// `(0x50 << 1) | 1` on x86_64 and `0x50` raw on aarch64.
pub const STATUS_FRAMEBUFFER_FAULT: u8 = 0x50;

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

// ARM DEN 0024A ("Semihosting for AArch32 and AArch64"): the SYS_EXIT
// operation number and the reason code QEMU maps to a normal process exit.
pub const SEMIHOSTING_SYS_EXIT: u32 = 0x18;
pub const SEMIHOSTING_ADP_STOPPED_APPLICATION_EXIT: u64 = 0x20026;

/// Exit code QEMU reports for a semihosting `SYS_EXIT` subcode: passed
/// through raw, unlike [`qemu_exit_code`]'s isa-debug-exit formula.
#[must_use]
pub const fn semihosting_exit_code(status: u8) -> i32 {
    status as i32
}

/// Terminates QEMU via semihosting `SYS_EXIT`; the emulator then exits with
/// [`semihosting_exit_code(status)`].
#[cfg(all(target_arch = "aarch64", not(test)))]
pub fn semihosting_exit(status: u8) -> ! {
    let mut args = [SEMIHOSTING_ADP_STOPPED_APPLICATION_EXIT, u64::from(status)];
    // SAFETY: ARM DEN 0024A defines SYS_EXIT as w0 = 0x18 with x1 pointing at
    // the {reason, subcode} block, trapped by `hlt #0xF000`; run.ps1 starts
    // QEMU with -semihosting-config enable=on so the trap terminates the
    // emulator before control returns.
    unsafe {
        core::arch::asm!(
            "mov w0, #{op}",
            "hlt #0xf000",
            op = const SEMIHOSTING_SYS_EXIT,
            in("x1") args.as_mut_ptr(),
            options(nostack)
        );
    }
    unreachable!("semihosting SYS_EXIT terminates QEMU before control returns");
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
        let statuses = [
            STATUS_BOOT_OK,
            STATUS_BOOT_INFO_MISSING,
            STATUS_UART_FAULT,
            STATUS_FRAMEBUFFER_MISSING,
            STATUS_FRAMEBUFFER_FAULT,
        ];
        for status in statuses {
            assert_eq!(qemu_exit_code(status) % 2, 1);
        }
        assert_eq!(qemu_exit_code(STATUS_UART_FAULT), 97);
        for i in 0..statuses.len() {
            for j in (i + 1)..statuses.len() {
                assert_ne!(qemu_exit_code(statuses[i]), qemu_exit_code(statuses[j]));
                assert_ne!(
                    semihosting_exit_code(statuses[i]),
                    semihosting_exit_code(statuses[j])
                );
            }
        }
    }

    #[test]
    fn documented_io_base_matches_qemu_default() {
        assert_eq!(DEBUG_EXIT_IO_BASE, 0x501);
    }

    #[test]
    fn semihosting_constants_match_arm_den_0024a() {
        assert_eq!(SEMIHOSTING_SYS_EXIT, 0x18);
        assert_eq!(SEMIHOSTING_ADP_STOPPED_APPLICATION_EXIT, 0x20026);
    }

    #[test]
    fn semihosting_exit_code_is_a_raw_pass_through() {
        assert_eq!(semihosting_exit_code(STATUS_BOOT_OK), 16);
        assert_eq!(semihosting_exit_code(STATUS_BOOT_INFO_MISSING), 32);
        assert_eq!(semihosting_exit_code(STATUS_UART_FAULT), 48);
        assert_eq!(semihosting_exit_code(u8::MAX), 255);
    }
}
