//! The sole home of `unsafe` in Ferric-K: entry assembly, drivers, locks,
//! Limine ABI, and the cfg-gated `arch/{x86_64,aarch64}` modules. The public
//! API stays 100% safe — misuse is prevented by types, not documentation.
//
// no_std dropped under cfg(test): the host test harness needs std.
#![cfg_attr(not(test), no_std)]
// linkage backs the weak mem* shims in `mem.rs`.
#![cfg_attr(not(test), feature(linkage))]

pub mod arch;
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
pub mod console;
pub mod framebuffer;
pub mod limine;
pub mod log;
#[cfg(all(
    target_os = "none",
    not(test),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub mod panic;
// Excluded on host: defining memcpy etc. would collide with the platform CRT
// when linking the test harness.
#[cfg(not(test))]
pub mod mem;
// Included in host test builds so its register semantics run against fake MMIO.
#[cfg(all(target_arch = "x86_64", not(test)))]
pub mod gdt;
#[cfg(all(target_arch = "x86_64", not(test)))]
pub mod idt;
#[cfg(all(any(target_arch = "x86_64", target_arch = "aarch64"), not(test)))]
pub mod interrupt;
// GICv2 + generic timer: register semantics run on the host; the asm-backed
// pieces are aarch64-only.
#[cfg(any(target_arch = "aarch64", test))]
pub mod gictimer;
// PIT + 8259A for x86_64; host-built because the I/O helpers compile there.
#[cfg(target_arch = "x86_64")]
pub mod pit;
#[cfg(any(target_arch = "aarch64", test))]
pub mod pl011;
#[cfg(target_arch = "x86_64")]
pub mod port;
pub mod qemu;
#[cfg(target_arch = "x86_64")]
pub mod serial;
pub mod sync;
pub mod text;
pub mod time;
pub mod volatile;
// Page-descriptor logic is host-tested like the drivers it supports.
#[cfg(any(target_arch = "aarch64", test))]
pub mod mmu;

/// Serial proof line emitted once the color-bar self-test has passed.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const FRAMEBUFFER_OK_MARKER: &str = "FRAMEBUFFER OK\n";

/// Wakes on the next interrupt: `hlt` on x86_64, `wfi` on aarch64; returns
/// as soon as one is delivered.
pub fn wait_for_interrupt() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: `hlt` parks the CPU until an interrupt arrives (Intel SDM
    // Vol. 2A); returns when one is delivered.
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `wfi` suspends until an interrupt wakes the CPU (ARM DDI 0487).
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    core::hint::spin_loop();
}

/// Common early-boot path after bootloader handoff; still on the bootloader
/// stack.  Sets up the serial and framebuffer globals, runs the colour-bar
/// self-test, then hands off to [`console::kmain`].
pub fn boot() -> ! {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        if !serial::init_global(serial::COM1_BASE) {
            qemu::debug_exit(qemu::STATUS_UART_FAULT);
        }

        gdt::init();
        idt::init();

        #[cfg(feature = "exception-on-boot")]
        {
            // Deliberate #DE via a raw `div` (not Rust's checked `/`, which
            // panics instead of faulting). 1/0 overflows the 64-bit EDX:RAX
            // quotient, so the CPU raises vector 0 (Intel SDM Vol. 2A §DIV).
            core::hint::black_box(1u64);
            unsafe {
                core::arch::asm!(
                    "mov rax, 1",
                    "xor edx, edx",
                    "xor ecx, ecx",
                    "div rcx",
                    options(nostack),
                );
            }
            unreachable!("divide-by-zero should have trapped");
        }

        #[cfg(not(feature = "exception-on-boot"))]
        {
            use ferric_api::TextSink;

            const BANNER_INFO_MISSING: &str = "Ferric-K x86_64\nBOOT INFO MISSING\n";

            crate::pit::init();
            crate::time::init_tsc(crate::limine::tsc_frequency());
            interrupt::revert_to_pic_mode();
            interrupt::enable();

            let Some(info) = limine::collect() else {
                serial::with_serial(|s| s.write_str(BANNER_INFO_MISSING));
                qemu::debug_exit(qemu::STATUS_BOOT_INFO_MISSING);
            };
            serial::with_serial(|s| s.write_str("Ferric-K x86_64\nBOOT OK\n"));

            if !framebuffer::init_from_boot_info(&info) {
                qemu::debug_exit(qemu::STATUS_FRAMEBUFFER_MISSING);
            }
            if !framebuffer::run_color_bar_self_test() {
                qemu::debug_exit(qemu::STATUS_FRAMEBUFFER_FAULT);
            }
            serial::with_serial(|s| s.write_str(FRAMEBUFFER_OK_MARKER));

            #[cfg(feature = "panic-on-boot")]
            {
                panic!("deliberate boot panic (panic-on-boot)")
            }
            #[cfg(not(feature = "panic-on-boot"))]
            console::kmain()
        }
    }

    #[cfg(all(target_arch = "aarch64", not(test)))]
    {
        use ferric_api::TextSink;

        let Some(info) = limine::collect() else {
            qemu::semihosting_exit(qemu::STATUS_BOOT_INFO_MISSING);
        };
        if !arch::aarch64::map_uart_window(info.hhdm_offset) {
            qemu::semihosting_exit(qemu::STATUS_UART_FAULT);
        }
        if !pl011::init_global(pl011::UART0_BASE.wrapping_add(info.hhdm_offset as usize)) {
            qemu::semihosting_exit(qemu::STATUS_UART_FAULT);
        }
        pl011::with_serial(|s| s.write_str("Ferric-K aarch64\nBOOT OK\n"));
        interrupt::init();

        #[cfg(feature = "exception-on-boot")]
        {
            // Deliberate `brk #0`: synchronous exception at EL1 with EC 0x3C
            // (AArch64 BRK), mirroring the x86_64 `div` test fault.
            // SAFETY: `brk #0` traps into the installed vector table, which
            // captures the frame and never returns to this point.
            unsafe {
                core::arch::asm!("brk #0", options(nostack));
            }
            unreachable!("brk should have trapped");
        }

        #[cfg(not(feature = "exception-on-boot"))]
        {
            if !gictimer::init(info.hhdm_offset) {
                qemu::semihosting_exit(qemu::STATUS_BOOT_INFO_MISSING);
            }
            interrupt::enable();

            if !framebuffer::init_from_boot_info(&info) {
                qemu::semihosting_exit(qemu::STATUS_FRAMEBUFFER_MISSING);
            }
            if !framebuffer::run_color_bar_self_test() {
                qemu::semihosting_exit(qemu::STATUS_FRAMEBUFFER_FAULT);
            }
            pl011::with_serial(|s| s.write_str(FRAMEBUFFER_OK_MARKER));

            #[cfg(feature = "panic-on-boot")]
            {
                panic!("deliberate boot panic (panic-on-boot)")
            }
            #[cfg(not(feature = "panic-on-boot"))]
            console::kmain()
        }
    }

    #[cfg(any(not(any(target_arch = "x86_64", target_arch = "aarch64")), test))]
    {
        // Validated but unused until output exists on this path.
        let _boot_info = limine::collect();
        halt()
    }
}

/// Parks the CPU forever: `hlt` on x86_64, `wfi` on aarch64.
pub fn halt() -> ! {
    #[cfg(target_arch = "x86_64")]
    loop {
        // SAFETY: `hlt` suspends the CPU until an interrupt; whichever state
        // interrupts are in, the loop re-issues it.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    loop {
        // SAFETY: `wfi` suspends the CPU until an interrupt wakes it.
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    loop {
        core::hint::spin_loop();
    }
}
