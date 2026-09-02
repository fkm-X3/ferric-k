//! Crash-time handler: formats the panic into a fixed buffer and dumps it as
//! a red console panel plus a serial line, then parks the CPU. Lives here so
//! the kernel bin stays unsafe-free; the public surface is safe.

use core::fmt;
use core::panic::PanicInfo;

/// Fixed-capacity byte sink for panic text (no heap, no float formatting).
struct PanicBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> PanicBuf<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    /// Consumes the writer, returning the collected text. Writes only ever
    /// append whole UTF-8 `&str` chunks (args are never clipped
    /// mid-codepoint), so the slice is always valid.
    fn into_str(self) -> &'a str {
        let len = self.len;
        let buf: &'a [u8] = self.buf;
        core::str::from_utf8(&buf[..len]).expect("panic buffer holds only UTF-8")
    }
}

impl fmt::Write for PanicBuf<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.buf.len() {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

const PANIC_BUF_CAP: usize = 512;

/// Kernel-wide panic handler. Only compiled for a real kernel link (`test`
/// builds use `std`'s handler via the harness; host build-std is absent, and
/// defining `panic_impl` there would collide with `std`'s).
#[cfg(all(
    target_os = "none",
    not(test),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut buf = [0u8; PANIC_BUF_CAP];
    let message = {
        use core::fmt::Write;
        let mut w = PanicBuf::new(&mut buf);
        let _ = writeln!(w, "KERNEL PANIC");
        if let Some(loc) = info.location() {
            let _ = writeln!(w, "{}:{}: {}", loc.file(), loc.line(), info.message());
        } else {
            let _ = writeln!(w, "{}", info.message());
        }
        w.into_str()
    };
    crate::console::render_panic(&[message]);
}
