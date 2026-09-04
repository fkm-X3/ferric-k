//! CPU exception handling: assembly stubs for x86_64 vectors, a common
//! diagnostic dump to serial, and an RAII interrupt-masking guard.

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

/// Write a pre-formatted byte slice directly to serial.
///
/// Bypasses the console lock and framebuffer to avoid re-entrancy issues.
/// Called only from exception/panic paths where the normal output path may
/// be locked or broken.
fn serial_write(s: &str) {
    #[cfg(target_arch = "x86_64")]
    crate::serial::with_serial(|serial| serial.write_str(s));
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

/// Read the exception frame from the pointer saved by the assembly stubs,
/// format a register dump, write it to serial, and halt the CPU.
///
/// This function never returns.
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
exc_stub_no_err!(exc_vector_0, 0); // #DE divide error
exc_stub_no_err!(exc_vector_1, 1); // #DB debug
exc_stub_no_err!(exc_vector_2, 2); // NMI
exc_stub_no_err!(exc_vector_3, 3); // #BP breakpoint
exc_stub_no_err!(exc_vector_4, 4); // #OF overflow
exc_stub_no_err!(exc_vector_5, 5); // #BR bound range
exc_stub_no_err!(exc_vector_6, 6); // #UD invalid opcode
exc_stub_no_err!(exc_vector_7, 7); // #NM device not available
exc_stub_no_err!(exc_vector_9, 9); // #MF x87 FPU error
exc_stub_no_err!(exc_vector_16, 16); // #MF x87 FPU alignment check
exc_stub_no_err!(exc_vector_18, 18); // #MC machine check
exc_stub_no_err!(exc_vector_20, 20); // #XM SIMD exception
exc_stub_no_err!(exc_vector_30, 30); // #CP control protection

// Vectors with error code (Intel SDM Vol. 3A §6.12, Table 6-1).
exc_stub_with_err!(exc_vector_13, 13); // #GP general protection
exc_stub_with_err!(exc_vector_14, 14); // #PF page fault

// ---------------------------------------------------------------------------
// IrqGuard: RAII interrupt masking via RFLAGS.IF.
// ---------------------------------------------------------------------------

/// RAII guard that saves the interrupt flag state on creation and restores it
/// on drop. Use [`IrqGuard::disable`] to mask interrupts for a critical
/// section; interrupts are unmasked when the guard goes out of scope.
///
/// `disable()` and the `Drop` impl use `cli`/`popfq` which are ring-0 only,
/// so they are gated to `target_os = "none"`.
pub struct IrqGuard {
    rflags: u64,
}

#[cfg(target_os = "none")]
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
        IrqGuard { rflags }
    }
}

impl IrqGuard {
    /// Returns whether interrupts were enabled before `disable()` was called.
    pub fn was_enabled(&self) -> bool {
        (self.rflags & (1 << 9)) != 0
    }
}

#[cfg(target_os = "none")]
impl Drop for IrqGuard {
    fn drop(&mut self) {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: `push {rflags}` / `popfq` restores the exact RFLAGS saved
        // by `disable()`, re-enabling interrupts if they were enabled before.
        unsafe {
            core::arch::asm!(
                "push {rflags}",
                "popfq",
                rflags = in(reg) self.rflags,
                options(nostack, preserves_flags),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_frame_size_and_align() {
        // 14 u64 fields = 112 bytes, 8-byte aligned.
        assert_eq!(core::mem::size_of::<ExceptionFrame>(), 112);
        assert_eq!(core::mem::align_of::<ExceptionFrame>(), 8);
    }
}
