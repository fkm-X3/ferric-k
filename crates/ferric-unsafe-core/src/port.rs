//! x86_64 port-mapped I/O: typed handles over the `in`/`out` instructions.

use core::marker::PhantomData;

/// Handle to one I/O port of register width `T`.
pub struct Port<T> {
    addr: u16,
    _width: PhantomData<T>,
}

impl<T> Port<T> {
    pub const fn new(addr: u16) -> Self {
        Self {
            addr,
            _width: PhantomData,
        }
    }

    pub const fn addr(&self) -> u16 {
        self.addr
    }
}

impl Port<u8> {
    pub fn read(&self) -> u8 {
        let value: u8;
        // SAFETY: `in` is legal only at ring 0+, which the kernel guarantees.
        // `nomem` is deliberately omitted so every read stays a visible side
        // effect and polling loops cannot be collapsed by the optimizer.
        unsafe {
            core::arch::asm!(
                "in al, dx",
                out("al") value,
                in("dx") self.addr,
                options(nostack, preserves_flags)
            );
        }
        value
    }

    pub fn write(&mut self, value: u8) {
        // SAFETY: `out` is legal only at ring 0+, which the kernel guarantees;
        // it targets a device register, never Rust memory.
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") self.addr,
                in("al") value,
                options(nostack, preserves_flags)
            );
        }
    }
}

impl Port<u16> {
    pub fn read(&self) -> u16 {
        let value: u16;
        // SAFETY: same ring-0 and side-effect reasoning as `Port<u8>::read`.
        unsafe {
            core::arch::asm!(
                "in ax, dx",
                out("ax") value,
                in("dx") self.addr,
                options(nostack, preserves_flags)
            );
        }
        value
    }

    pub fn write(&mut self, value: u16) {
        // SAFETY: same ring-0 reasoning as `Port<u8>::write`; device-register
        // access only.
        unsafe {
            core::arch::asm!(
                "out dx, ax",
                in("dx") self.addr,
                in("ax") value,
                options(nostack, preserves_flags)
            );
        }
    }
}

impl Port<u32> {
    pub fn read(&self) -> u32 {
        let value: u32;
        // SAFETY: same ring-0 and side-effect reasoning as `Port<u8>::read`.
        unsafe {
            core::arch::asm!(
                "in eax, dx",
                out("eax") value,
                in("dx") self.addr,
                options(nostack, preserves_flags)
            );
        }
        value
    }

    pub fn write(&mut self, value: u32) {
        // SAFETY: same ring-0 reasoning as `Port<u8>::write`; device-register
        // access only.
        unsafe {
            core::arch::asm!(
                "out dx, eax",
                in("dx") self.addr,
                in("eax") value,
                options(nostack, preserves_flags)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn handles_are_register_sized() {
        assert_eq!(size_of::<Port<u8>>(), 2);
        assert_eq!(size_of::<Port<u16>>(), 2);
        assert_eq!(size_of::<Port<u32>>(), 2);
        assert_eq!(align_of::<Port<u8>>(), 2);
        assert_eq!(align_of::<Port<u32>>(), 2);
    }

    #[test]
    fn new_stores_the_port_address() {
        assert_eq!(Port::<u8>::new(0x3f8).addr(), 0x3f8);
        assert_eq!(Port::<u16>::new(0x0cf8).addr(), 0x0cf8);
    }
}
