//! CPU exception handling: assembly stubs for x86_64 vectors, the aarch64
//! exception vector table and save stubs, a common diagnostic dump to serial,
//! and an RAII interrupt-masking guard.

use core::fmt;
use core::fmt::Write;
#[cfg(target_os = "none")]
use core::sync::atomic::{Ordering, compiler_fence};

use ferric_api::TextSink;

/// CPU register state pushed by exception stubs plus the interrupt frame
/// pushed by the CPU (Intel SDM Vol. 3A §6.12.1, Fig. 6-4).
///
/// For vectors without error code, the `error_code` field is a dummy zero
/// pushed by the stub. For vectors with error code (13, 14), it is the
/// real value from the CPU.
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExceptionFrame {
    // Pushed by stub (last push first in memory): rax lands lowest.
    rax: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    vector: u64,
    error_code: u64,
    // Pushed by CPU (same-CPL interrupt frame, RIP lowest; SDM Fig. 6-4).
    rip: u64,
    cs: u64,
    rflags: u64,
}

/// CPU state saved by the aarch64 vector-table stubs for one exception (ARM
/// DDI 0487, "AArch64 exception entry"). Layout matches the stub's save
/// sequence so the frame is a direct view over the pushed stack region.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExceptionFrame {
    /// X0..X30 at the exception point.
    x: [u64; 31],
    /// Stack pointer value at exception entry.
    sp: u64,
    /// Exception link register (instruction address plus reason bits).
    elr: u64,
    /// Saved PSTATE.
    spsr: u64,
    /// Exception syndrome register.
    esr: u64,
    /// Fault address register.
    far: u64,
    /// Classifier from the taking stub: 0 sync, 1 irq, 2 fiq, 3 serror.
    kind: u64,
}

/// Write a pre-formatted byte slice directly to serial.
///
/// Bypasses the console lock and framebuffer to avoid re-entrancy issues.
/// Called only from exception/panic paths where the normal output path may
/// be locked or broken. Takes the serial lock only if it is immediately
/// available; otherwise writes raw to the port so a dump that fires inside a
/// print cannot deadlock on the spinlock the interrupted write holds.
fn serial_write(s: &str) {
    #[cfg(target_arch = "x86_64")]
    {
        let taken = crate::serial::with_serial_try(|serial| serial.write_str(s));
        if !taken {
            crate::serial::write_emergency(s);
        }
    }
    #[cfg(target_arch = "aarch64")]
    crate::pl011::with_serial(|serial| serial.write_str(s));
}

/// Adapter over a byte buffer implementing `fmt::Write` for exception
/// diagnostics. Mirrors the panic-handler pattern (no heap, no floats).
struct BufWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl fmt::Write for BufWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let end = self.pos + bytes.len();
        if end <= self.buf.len() {
            self.buf[self.pos..end].copy_from_slice(bytes);
            self.pos = end;
        }
        Ok(())
    }
}

/// Format the exception register dump into `buf` and return the written slice.
#[cfg(target_arch = "x86_64")]
fn format_exception<'a>(buf: &'a mut [u8], frame: &ExceptionFrame) -> &'a str {
    let pos = {
        let mut w = BufWriter { buf, pos: 0 };
        let _ = writeln!(
            w,
            "EXCEPTION: vector {}, error code 0x{:X}",
            frame.vector, frame.error_code
        );
        let _ = writeln!(
            w,
            "  RAX={:016X} RCX={:016X} RDX={:016X}",
            frame.rax, frame.rcx, frame.rdx
        );
        let _ = writeln!(
            w,
            "  RSI={:016X} RDI={:016X} R11={:016X}",
            frame.rsi, frame.rdi, frame.r11
        );
        let _ = writeln!(
            w,
            "  R8 ={:016X} R9 ={:016X} R10={:016X}",
            frame.r8, frame.r9, frame.r10
        );
        let _ = writeln!(
            w,
            "  RIP={:016X} CS={:04X} RFLAGS={:016X}",
            frame.rip, frame.cs, frame.rflags
        );
        let _ = writeln!(w, "KERNEL HALT");
        let _ = w.write_str("---\n");
        w.pos
    };
    // SAFETY: `pos` is always <= buf.len(); slicing is valid.
    core::str::from_utf8(&buf[..pos]).unwrap_or("exception (utf8 error)")
}

/// Format an aarch64 exception dump into `buf` and return the written slice.
#[cfg(target_arch = "aarch64")]
fn format_exception<'a>(buf: &'a mut [u8], frame: &ExceptionFrame) -> &'a str {
    let pos = {
        let mut w = BufWriter { buf, pos: 0 };
        let kind = match frame.kind {
            0 => "SYNC",
            1 => "IRQ",
            2 => "FIQ",
            _ => "SERROR",
        };
        let _ = writeln!(
            w,
            "EXCEPTION: kind {kind}, ESR 0x{:08X} (EC {:#X})",
            frame.esr,
            (frame.esr >> 26) & 0x3F
        );
        for start in (0..28).step_by(4) {
            let _ = writeln!(
                w,
                "  X{:<2}={:016X} X{:<2}={:016X} X{:<2}={:016X} X{:<2}={:016X}",
                start,
                frame.x[start],
                start + 1,
                frame.x[start + 1],
                start + 2,
                frame.x[start + 2],
                start + 3,
                frame.x[start + 3],
            );
        }
        let _ = writeln!(
            w,
            "  X28={:016X} X29={:016X} X30={:016X}",
            frame.x[28], frame.x[29], frame.x[30]
        );
        let _ = writeln!(
            w,
            "  SP ={:016X} ELR={:016X} SPSR={:08X}",
            frame.sp, frame.elr, frame.spsr
        );
        let _ = writeln!(w, "  ESR={:016X} FAR={:016X}", frame.esr, frame.far);
        let _ = writeln!(w, "KERNEL HALT");
        let _ = w.write_str("---\n");
        w.pos
    };
    // SAFETY: `pos` is always <= buf.len(); slicing is valid.
    core::str::from_utf8(&buf[..pos]).unwrap_or("exception (utf8 error)")
}

/// Read the exception frame from the pointer saved by the assembly stubs,
/// format a register dump, write it to serial, and halt the CPU.
///
/// This function never returns.
#[cfg(target_arch = "x86_64")]
unsafe extern "sysv64" fn exception_common(frame: *const ExceptionFrame) -> ! {
    // SAFETY: `frame` points to the ExceptionFrame pushed by the assembly
    // stubs on the current CPU stack; its layout and lifetime are guaranteed
    // by the stub contract. The pointer is valid for the duration of this call.
    let frame = unsafe { &*frame };
    let mut buf = [0u8; 1024];
    let msg = format_exception(&mut buf, frame);
    serial_write(msg);
    crate::halt()
}

/// Naked trampoline: captures the frame pointer, aligns the stack, and
/// tail-calls into the safe `exception_common` handler. Never returns.
///
/// The `call` instruction pushes a return address (8 bytes), making RSP
/// 16-byte aligned per the SysV-64 ABI when `exception_common` is entered.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "sysv64" fn common_handler_wrapper() -> ! {
    // SAFETY: naked function contains only inline assembly; no Rust
    // code is generated. The stack frame layout matches ExceptionFrame.
    core::arch::naked_asm!(
        "mov r12, rdi",
        "and rsp, -16",
        "call {handler}",
        handler = sym exception_common,
    );
}

// ---------------------------------------------------------------------------
// Assembly stubs: one per exception vector.
//
// Vectors WITHOUT error code (0–9, 16, 18, 20, 30): push a dummy 0 so the
// stack layout is uniform. Vectors WITH error code (13, 14): skip the dummy
// push — the CPU already placed the error code on the stack.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
macro_rules! exc_stub_no_err {
    ($name:ident, $vector:expr) => {
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "sysv64" fn $name() -> ! {
            core::arch::naked_asm!(
                "push 0",
                concat!("push ", stringify!($vector)),
                "push r11",
                "push r10",
                "push r9",
                "push r8",
                "push rdi",
                "push rsi",
                "push rdx",
                "push rcx",
                "push rax",
                "mov rdi, rsp",
                "call {tramp}",
                tramp = sym common_handler_wrapper,
            );
        }
    };
}

#[cfg(target_arch = "x86_64")]
macro_rules! exc_stub_with_err {
    ($name:ident, $vector:expr) => {
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "sysv64" fn $name() -> ! {
            core::arch::naked_asm!(
                concat!("push ", stringify!($vector)),
                "push r11",
                "push r10",
                "push r9",
                "push r8",
                "push rdi",
                "push rsi",
                "push rdx",
                "push rcx",
                "push rax",
                "mov rdi, rsp",
                "call {tramp}",
                tramp = sym common_handler_wrapper,
            );
        }
    };
}

// Vectors without error code (Intel SDM Vol. 3A §6.12, Table 6-1).
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_0, 0); // #DE divide error
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_1, 1); // #DB debug
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_2, 2); // NMI
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_3, 3); // #BP breakpoint
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_4, 4); // #OF overflow
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_5, 5); // #BR bound range
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_6, 6); // #UD invalid opcode
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_7, 7); // #NM device not available
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_9, 9); // #MF x87 FPU error
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_16, 16); // #MF x87 FPU alignment check
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_18, 18); // #MC machine check
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_20, 20); // #XM SIMD exception
#[cfg(target_arch = "x86_64")]
exc_stub_no_err!(exc_vector_30, 30); // #CP control protection

// Vectors with error code (Intel SDM Vol. 3A §6.12, Table 6-1).
#[cfg(target_arch = "x86_64")]
exc_stub_with_err!(exc_vector_13, 13); // #GP general protection
#[cfg(target_arch = "x86_64")]
exc_stub_with_err!(exc_vector_14, 14); // #PF page fault

// ---------------------------------------------------------------------------
// IRQ0 (PIT timer) stub: a distinct handler because it must return. Gates
// are 64-bit interrupt type, so IF is cleared on entry and the handler runs
// atomically with respect to further IRQs — it never re-enables, which also
// means no nested deliveries can clobber the frame or the stack below RSP.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "sysv64" fn irq_stub_irq0() -> ! {
    core::arch::naked_asm!(
        "push 0",
        "push 0x20",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rax",
        "mov rdi, rsp",
        "call {tramp}",
        "pop rax",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "add rsp, 16",
        "iretq",
        tramp = sym irq_handler_wrapper,
    );
}

/// Naked trampoline: saves the incoming RSP in r12 and the frame pointer in
/// r13 (both callee-saved, preserved across the Rust call), aligns RSP for the
/// SysV-64 ABI, calls the Rust handler, then restores both and returns into
/// the stub.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "sysv64" fn irq_handler_wrapper() -> usize {
    core::arch::naked_asm!(
        "mov r12, rsp",
        "mov r13, rdi",
        "and rsp, -16",
        "call {handler}",
        "mov rsp, r12",
        "ret",
        handler = sym irq_common,
    );
}

/// Dispatches an IRQ frame: the PIT handler for IRQ0, any other vector
/// falls through to the exception dump-and-halt path.
#[cfg(target_arch = "x86_64")]
unsafe extern "sysv64" fn irq_common(frame: *const ExceptionFrame) -> usize {
    // SAFETY: `frame` points at the ExceptionFrame pushed by `irq_stub_irq0`
    // on the current stack; the stub contract keeps it valid for this call.
    let frame = unsafe { &*frame };
    if frame.vector == crate::pit::IRQ0_VECTOR as u64 {
        crate::pit::handle_timer_irq();
    } else {
        let mut buf = [0u8; 1024];
        let msg = format_exception(&mut buf, frame);
        serial_write(msg);
        crate::halt();
    }
    0
}

// ---------------------------------------------------------------------------
// aarch64 vector table and save stubs.
//
// VBAR_EL1 must hold the start of a 2048-byte table (ARM DDI 0487, "Exception
// Vector Table"); the naked fn below emits the table plus four per-class
// stubs as one assembly block. All sixteen entries route to the dump-and-halt
// path, so the kernel never continues memory-unsafely from a fault.
// ---------------------------------------------------------------------------

// Address of the asm label `exception_vector_table` for [`crate::interrupt::init`];
// defined by the naked fn below and linked here so `init` can install it.
#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    fn exception_vector_table();
}

/// Emits the exception vector table and per-class save stubs as one assembly
/// block. `.balign`/`.global` place `exception_vector_table` at the
/// architectural VBAR alignment; each entry branches to its class stub, which
/// captures the [`ExceptionFrame`] layout and transfers to
/// [`exception_common`]. `exception_common` never returns, so the stubs end on
/// a shared `hang` loop as a safety net.
///
/// # Safety
/// The function body is raw assembly with no Rust prologue/epilogue; it must
/// not be called from Rust — only branched into via the vector table.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub unsafe extern "C" fn exception_vector_tables() -> ! {
    // SAFETY: naked function contains only inline assembly; the directives
    // and instructions define the vector table contract in full.
    core::arch::naked_asm!(
        ".balign 2048",
        ".global exception_vector_table",
        "exception_vector_table:",
        // EL1t group, SP_EL0.
        "b exc_sync",
        ".balign 0x80",
        "b exc_irq",
        ".balign 0x80",
        "b exc_fiq",
        ".balign 0x80",
        "b exc_serror",
        ".balign 0x80",
        // EL1h group, SP_EL1. IRQs land here (kernel runs SPSel=1) and go
        // through `exc_irq_ret`, which returns from the interrupt instead of
        // dumping.
        "b exc_sync",
        ".balign 0x80",
        "b exc_irq_ret",
        ".balign 0x80",
        "b exc_fiq",
        ".balign 0x80",
        "b exc_serror",
        ".balign 0x80",
        // EL0 group, AArch64.
        "b exc_sync",
        ".balign 0x80",
        "b exc_irq",
        ".balign 0x80",
        "b exc_fiq",
        ".balign 0x80",
        "b exc_serror",
        ".balign 0x80",
        // EL0 group, AArch32.
        "b exc_sync",
        ".balign 0x80",
        "b exc_irq",
        ".balign 0x80",
        "b exc_fiq",
        ".balign 0x80",
        "b exc_serror",
        // Zero-fill the table tail: stray execution hits `udf #0` and faults
        // into the sync handler instead of running the stubs below.
        ".balign 2048, 0",
        ".macro EXC_HANDLER label, kind",
        "\\label:",
        "sub sp, sp, #304",
        "stp x0, x1, [sp, #0]",
        "stp x2, x3, [sp, #16]",
        "stp x4, x5, [sp, #32]",
        "stp x6, x7, [sp, #48]",
        "stp x8, x9, [sp, #64]",
        "stp x10, x11, [sp, #80]",
        "stp x12, x13, [sp, #96]",
        "stp x14, x15, [sp, #112]",
        "stp x16, x17, [sp, #128]",
        "stp x18, x19, [sp, #144]",
        "stp x20, x21, [sp, #160]",
        "stp x22, x23, [sp, #176]",
        "stp x24, x25, [sp, #192]",
        "stp x26, x27, [sp, #208]",
        "stp x28, x29, [sp, #224]",
        "str x30, [sp, #240]",
        "add x16, sp, #304",
        "str x16, [sp, #248]",
        "mrs x17, elr_el1",
        "str x17, [sp, #256]",
        "mrs x18, spsr_el1",
        "str x18, [sp, #264]",
        "mrs x19, esr_el1",
        "str x19, [sp, #272]",
        "mrs x20, far_el1",
        "str x20, [sp, #280]",
        "mov x21, #\\kind",
        "str x21, [sp, #288]",
        "mov x0, sp",
        "bl exception_common",
        "b hang",
        ".endm",
        "EXC_HANDLER exc_sync, 0",
        "EXC_HANDLER exc_irq, 1",
        "EXC_HANDLER exc_fiq, 2",
        "EXC_HANDLER exc_serror, 3",
        // IRQ return stub: identical register capture, but hands the frame to
        // `irq_common` (no deref, just the timer ack/re-arm) and then eret
        // back to the interrupted instruction with the saved ELR/SPSR.
        ".macro IRQ_HANDLER label",
        "\\label:",
        "sub sp, sp, #304",
        "stp x0, x1, [sp, #0]",
        "stp x2, x3, [sp, #16]",
        "stp x4, x5, [sp, #32]",
        "stp x6, x7, [sp, #48]",
        "stp x8, x9, [sp, #64]",
        "stp x10, x11, [sp, #80]",
        "stp x12, x13, [sp, #96]",
        "stp x14, x15, [sp, #112]",
        "stp x16, x17, [sp, #128]",
        "stp x18, x19, [sp, #144]",
        "stp x20, x21, [sp, #160]",
        "stp x22, x23, [sp, #176]",
        "stp x24, x25, [sp, #192]",
        "stp x26, x27, [sp, #208]",
        "stp x28, x29, [sp, #224]",
        "str x30, [sp, #240]",
        "add x16, sp, #304",
        "str x16, [sp, #248]",
        "mrs x17, elr_el1",
        "str x17, [sp, #256]",
        "mrs x18, spsr_el1",
        "str x18, [sp, #264]",
        "mrs x19, esr_el1",
        "str x19, [sp, #272]",
        "mrs x20, far_el1",
        "str x20, [sp, #280]",
        "mov x21, #1",
        "str x21, [sp, #288]",
        "mov x0, sp",
        "bl irq_common",
        "ldp x0, x1, [sp, #0]",
        "ldp x2, x3, [sp, #16]",
        "ldp x4, x5, [sp, #32]",
        "ldp x6, x7, [sp, #48]",
        "ldp x8, x9, [sp, #64]",
        "ldp x10, x11, [sp, #80]",
        "ldp x12, x13, [sp, #96]",
        "ldp x14, x15, [sp, #112]",
        "ldp x16, x17, [sp, #128]",
        "ldp x18, x19, [sp, #144]",
        "ldp x20, x21, [sp, #160]",
        "ldp x22, x23, [sp, #176]",
        "ldp x24, x25, [sp, #192]",
        "ldp x26, x27, [sp, #208]",
        "ldp x28, x29, [sp, #224]",
        "ldr x30, [sp, #240]",
        "add sp, sp, #304",
        "eret",
        ".endm",
        "IRQ_HANDLER exc_irq_ret",
        "hang:",
        "b hang",
    );
}

/// Acknowledges and services the pending IRQ, returning into the vector
/// epoxy. The frame pointer is currently unused on aarch64 — the handler
/// only touches the GIC — but it is passed for parity with the dump path.
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn irq_common(_frame: *const ExceptionFrame) {
    crate::gictimer::handle_timer_irq();
}

/// Format the dump the stubs captured, write it to serial, and halt the CPU.
///
/// # Safety
/// `frame` must point to the [`ExceptionFrame`] the vector-table stubs push
/// on the current CPU stack; the stub contract guarantees its layout and
/// lifetime for the duration of this (never-returning) call.
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exception_common(frame: *const ExceptionFrame) -> ! {
    // SAFETY: the stub passes a pointer to its own freshly pushed frame,
    // valid and readable for the whole call.
    let frame = unsafe { &*frame };
    let mut buf = [0u8; 1024];
    let msg = format_exception(&mut buf, frame);
    serial_write(msg);
    crate::halt()
}

/// Installs the vector table into `VBAR_EL1`; must run once before interrupts
/// are enabled so deliveries land in the capture stubs.
#[cfg(target_arch = "aarch64")]
pub fn init() {
    // SAFETY: `exception_vector_table` comes from the naked fn in this module
    // and is 2048-byte aligned (ARM DDI 0487, VBAR_EL1); the write is legal at
    // EL1 and at EL2+VHE, where the register redirects to VBAR_EL2.
    unsafe {
        core::arch::asm!(
            "msr vbar_el1, {base}",
            base = in(reg) exception_vector_table as *const () as usize,
            options(nostack, preserves_flags),
        );
    }
    let vbar: u64;
    // SAFETY: `mrs vbar_el1` reads back the value just installed.
    unsafe {
        core::arch::asm!(
            "mrs {vbar}, vbar_el1",
            vbar = out(reg) vbar,
            options(nomem, nostack, preserves_flags),
        );
    }
    debug_assert_eq!(vbar % 2048, 0);
}

/// Disables the local APIC so external interrupts take the legacy 8259 path
/// (PIC mode). The timer, keyboard, and every PMI on the BSP reach the CPU
/// via the 8259 in this configuration; required before `enable()`, because a
/// software-enabled LAPIC masks the PIC's INTR line and interrupts are lost.
#[cfg(target_arch = "x86_64")]
pub fn revert_to_pic_mode() {
    const IA32_APIC_BASE: u32 = 0x1B;
    let mut lo: u32;
    let mut hi: u32;
    // SAFETY: rdmsr/wrmsr of IA32_APIC_BASE (Intel SDM Vol. 3A §10.4.3);
    // single-threaded early boot, CPL 0. Bit 11 is APIC global enable.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE,
            out("eax") lo,
            out("edx") hi,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_APIC_BASE,
            in("eax") lo & !(1 << 11),
            in("edx") hi,
            options(nostack, preserves_flags),
        );
    }
}

/// Unmasks maskable interrupts: `sti` on x86_64, DAIF.I clear on aarch64.
/// Call once the CPU's vector table, IDT, and timer sources are installed.
#[cfg(target_arch = "x86_64")]
pub fn enable() {
    // SAFETY: `sti` clears IF; interrupts are only safely delivered once the
    // IDT and PIT are installed, which boot guarantees before calling this.
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_arch = "aarch64")]
pub fn enable() {
    // SAFETY: `msr daifclr, #4` clears only the DAIF I bit, unmasking IRQs;
    // boot installs the vector table and GIC timer first.
    unsafe {
        core::arch::asm!("msr daifclr, #4", options(nostack, preserves_flags));
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// IrqGuard: RAII interrupt masking via RFLAGS.IF (x86_64) or DAIF.I (aarch64).
// ---------------------------------------------------------------------------

/// RAII guard that saves the interrupt-mask state on creation and restores it
/// on drop. Use [`IrqGuard::disable`] to mask interrupts for a critical
/// section; interrupts are unmasked when the guard goes out of scope.
///
/// `disable()` and the `Drop` impl use ring-0/exception-mask instructions, so
/// they are gated to `target_os = "none"`.
pub struct IrqGuard {
    flags: u64,
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
impl IrqGuard {
    /// Saves RFLAGS and clears IF (bit 9) to disable maskable interrupts.
    ///
    /// Returns a guard that restores the original RFLAGS on drop.
    pub fn disable() -> Self {
        let rflags: u64;
        // SAFETY: `pushfq`/`popfq` are ring-0 safe and read/write the
        // full RFLAGS register. `cli` clears IF only; no other bits are
        // modified (Intel SDM Vol. 2A §3.2.1).
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {rflags}",
                "cli",
                rflags = out(reg) rflags,
                options(nostack, preserves_flags),
            );
        }
        compiler_fence(Ordering::SeqCst);
        IrqGuard { flags: rflags }
    }
}

#[cfg(all(target_os = "none", target_arch = "aarch64"))]
impl IrqGuard {
    /// Saves DAIF and masks IRQs by setting the I bit (bit 7).
    ///
    /// Returns a guard that restores the original DAIF on drop.
    pub fn disable() -> Self {
        let daif: u64;
        // SAFETY: `mrs daif` reads the whole exception-mask register;
        // `msr daifset, #4` sets only the IRQ mask bit I — the immediate
        // encodes D=1,A=2,I=4,F=8 (ARM DDI 0487, DAIFSET).
        unsafe {
            core::arch::asm!(
                "mrs {daif}, daif",
                "msr daifset, #4",
                daif = out(reg) daif,
                options(nostack, preserves_flags),
            );
        }
        compiler_fence(Ordering::SeqCst);
        IrqGuard { flags: daif }
    }
}

#[cfg(target_arch = "x86_64")]
impl IrqGuard {
    /// Returns whether interrupts were enabled before `disable()` was called.
    pub fn was_enabled(&self) -> bool {
        (self.flags & (1 << 9)) != 0
    }
}

#[cfg(target_arch = "aarch64")]
impl IrqGuard {
    /// Returns whether interrupts were enabled before `disable()` was called.
    pub fn was_enabled(&self) -> bool {
        (self.flags & (1 << 7)) == 0
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
impl Drop for IrqGuard {
    fn drop(&mut self) {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: `push {flags}` / `popfq` restores the exact RFLAGS saved
        // by `disable()`, re-enabling interrupts if they were enabled before.
        unsafe {
            core::arch::asm!(
                "push {flags}",
                "popfq",
                flags = in(reg) self.flags,
                options(nostack, preserves_flags),
            );
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "aarch64"))]
impl Drop for IrqGuard {
    fn drop(&mut self) {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: `msr daif` restores the exact DAIF saved by `disable()`,
        // re-enabling interrupts if they were enabled before.
        unsafe {
            core::arch::asm!(
                "msr daif, {flags}",
                flags = in(reg) self.flags,
                options(nostack, preserves_flags),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_64_frame_fits_the_stub_layout() {
        assert_eq!(core::mem::size_of::<ExceptionFrame>(), 112);
        assert_eq!(core::mem::align_of::<ExceptionFrame>(), 8);
    }

    #[test]
    fn aarch64_frame_fits_the_stub_layout() {
        assert_eq!(core::mem::size_of::<ExceptionFrame>(), 296);
        assert_eq!(core::mem::align_of::<ExceptionFrame>(), 8);
    }
}
