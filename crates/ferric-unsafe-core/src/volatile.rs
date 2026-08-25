//! Volatile access wrappers for memory-mapped registers: every read and
//! write stays an explicit side effect the optimizer cannot elide, merge,
//! or reorder relative to other volatile operations.

/// A handle to one register-sized location accessed only via volatile ops.
///
/// `ptr` must be valid and aligned for `T` for as long as the handle lives,
/// and must reference storage that tolerates volatile access (MMIO or
/// device-exposed memory).
pub struct Volatile<T> {
    ptr: *mut T,
}

impl<T: Copy> Volatile<T> {
    pub const fn new(ptr: *mut T) -> Self {
        Self { ptr }
    }

    pub const fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    /// Performs a single volatile read.
    pub fn read(&self) -> T {
        // SAFETY: the contract of `Volatile` requires `ptr` to be valid and
        // aligned for `T`; volatile reads of MMIO are defined behavior.
        unsafe { self.ptr.read_volatile() }
    }

    /// Performs a single volatile write.
    pub fn write(&mut self, value: T) {
        // SAFETY: same validity contract as `read`; volatile writes target
        // device registers, never aliased Rust memory.
        unsafe { self.ptr.write_volatile(value) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn handles_are_pointer_sized() {
        assert_eq!(size_of::<Volatile<u32>>(), size_of::<*mut u32>());
        assert_eq!(align_of::<Volatile<u32>>(), align_of::<*mut u32>());
    }

    #[test]
    fn is_const_constructible_and_exposes_its_address() {
        const NULL_HANDLE: Volatile<u32> = Volatile::new(core::ptr::null_mut());
        assert!(NULL_HANDLE.as_ptr().is_null());

        let mut cell = 7u32;
        let handle = Volatile::new(&mut cell as *mut u32);
        assert_eq!(handle.as_ptr(), &mut cell as *mut u32);
    }

    #[test]
    fn write_then_read_round_trips_every_register_width() {
        let mut b8 = 0u8;
        let mut b16 = 0u16;
        let mut b32 = 0u32;
        let mut b64 = 0u64;

        Volatile::new(&mut b8 as *mut u8).write(0xAB);
        Volatile::new(&mut b16 as *mut u16).write(0xABCD);
        Volatile::new(&mut b32 as *mut u32).write(0xDEAD_BEEF);
        Volatile::new(&mut b64 as *mut u64).write(0x0123_4567_89AB_CDEF);

        assert_eq!(b8, 0xAB);
        assert_eq!(b16, 0xABCD);
        assert_eq!(b32, 0xDEAD_BEEF);
        assert_eq!(b64, 0x0123_4567_89AB_CDEF);

        assert_eq!(Volatile::new(&mut b8 as *mut u8).read(), 0xAB);
        assert_eq!(
            Volatile::new(&mut b64 as *mut u64).read(),
            0x0123_4567_89AB_CDEF
        );
    }

    #[test]
    fn distinct_locations_stay_independent() {
        let mut a = 1u32;
        let mut b = 2u32;
        let mut ha = Volatile::new(&mut a as *mut u32);
        let hb = Volatile::new(&mut b as *mut u32);
        ha.write(10);
        assert_eq!(hb.read(), 2);
        assert_eq!(a, 10);
    }

    #[test]
    fn two_handles_to_one_location_observe_each_other() {
        let mut cell = 0u32;
        let base = &mut cell as *mut u32;
        let mut h1 = Volatile::new(base);
        let h2 = Volatile::new(base);
        h1.write(42);
        assert_eq!(h2.read(), 42);
    }
}
