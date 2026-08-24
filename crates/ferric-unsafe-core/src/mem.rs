//! In-tree `mem*` runtime functions.

use core::ffi::c_void;

#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let (dest, src) = (dest.cast::<u8>(), src.cast::<u8>());
    let mut i = 0usize;
    while i < n {
        // SAFETY: loop bound `i < n` plus the caller's contract above keep
        // every access inside the two regions.
        unsafe {
            *dest.add(i) = *src.add(i);
        }
        i += 1;
    }
    dest.cast()
}

/// C `memmove`: like [`memcpy`], but correct for overlapping regions.
///
/// # Safety
/// Caller guarantees both regions are valid for their accesses (C contract).
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let (dest, src) = (dest.cast::<u8>(), src.cast::<u8>());
    if dest as usize <= src as usize {
        let mut i = 0usize;
        while i < n {
            // SAFETY: bounded by `n` and the region-validity contract.
            unsafe {
                *dest.add(i) = *src.add(i);
            }
            i += 1;
        }
    } else {
        // Copy from the end so no source byte is read after being overwritten.
        let mut i = n;
        while i > 0 {
            // SAFETY: bounded by `n`; descending order preserves overlap safety.
            unsafe {
                *dest.add(i - 1) = *src.add(i - 1);
            }
            i -= 1;
        }
    }
    dest.cast()
}

/// C `memset`: fill `n` bytes at `dest` with `value`.
///
/// # Safety
/// Caller guarantees `dest..dest+n` is valid for writes, per the C contract.
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn memset(dest: *mut c_void, value: i32, n: usize) -> *mut c_void {
    let dest = dest.cast::<u8>();
    let mut i = 0usize;
    while i < n {
        // SAFETY: bounded by `n` and the region-validity contract.
        unsafe {
            *dest.add(i) = value as u8;
        }
        i += 1;
    }
    dest.cast()
}

/// C `memcmp`: compare `n` bytes; result mirrors the first difference
/// (unsigned byte comparison).
///
/// # Safety
/// Caller guarantees both regions are valid for reads (C contract).
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    let (a, b) = (a.cast::<u8>(), b.cast::<u8>());
    let mut i = 0usize;
    while i < n {
        // SAFETY: bounded by `n` and the region-validity contract.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
        i += 1;
    }
    0
}
