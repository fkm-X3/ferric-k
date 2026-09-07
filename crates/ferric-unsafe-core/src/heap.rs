//! In-tree heap: a first-fit free-list block allocator over a region sized
//! from the Limine memory map, guarded by the crate's `Spinlock`.

use crate::sync::Spinlock;
use core::alloc::Layout;

// `GlobalAlloc` and `UnsafeCell` are only needed by the kernel-only
// `#[global_allocator]` and static-arena pieces, which host builds disable.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
use core::alloc::GlobalAlloc;
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
use core::cell::UnsafeCell;

/// Metadata every block starts with. `size` bounds the block for carve and
/// coalesce; `next`/`prev` thread the free list (stale while allocated).
#[repr(C)]
struct Header {
    size: usize,
    next: *mut Header,
    prev: *mut Header,
}

/// An `usize` slot always kept just below an allocated payload, holding the
/// padding from the block's header to its payload so `dealloc` can recover
/// the header through `payload - HEADER_SIZE - offset`.
const HEADER_GAP: usize = size_of::<usize>();

const HEADER_SIZE: usize = core::mem::size_of::<Header>();

/// A leftover must hold a header plus user bytes to be worth splitting;
/// smaller tails stay glued to the allocation.
const MIN_BLOCK: usize = HEADER_SIZE * 2;

const fn align_up(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

/// Block allocator state. All mutations happen under the crate `Spinlock`
/// (including the `#[global_allocator]` paths below).
pub(crate) struct Heap {
    free: *mut Header,
    start: usize,
    end: usize,
}

impl Heap {
    pub(crate) const fn new() -> Self {
        Self {
            free: core::ptr::null_mut(),
            start: 0,
            end: 0,
        }
    }

    /// Replaces the free list with one free block spanning
    /// `[start, start + size)`; false when the region is unusable.
    fn init(&mut self, start: usize, size: usize) -> bool {
        let start = align_up(start, 8);
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        let end = end & !7;
        let size = end - start;
        if size < MIN_BLOCK {
            return false;
        }
        let head = start as *mut Header;
        // SAFETY: the caller guarantees `start..end` is live, writable,
        // never-moved memory (a memory-map region or the static arena); this
        // writes only the header at its head.
        unsafe {
            (*head).size = size;
            (*head).next = core::ptr::null_mut();
            (*head).prev = core::ptr::null_mut();
        }
        self.free = head;
        self.start = start;
        self.end = end;
        true
    }

    /// Allocates `layout` from the first free block that fits, returning null
    /// when none does (the runtime turns that into `handle_alloc_error`).
    /// Zero-size requests get a real block so every returned pointer is non-null
    /// and distinct.
    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = core::cmp::max(layout.size(), 1);
        let align = layout.align();
        let mut current = self.free;
        while !current.is_null() {
            // SAFETY: `current` only walks over live free-list nodes this
            // allocator created; `size`/`next` describe block extents that
            // stay inside `self.start..self.end`.
            let node = unsafe { &*current };
            let block_addr = current as usize;
            let block_size = node.size;
            // The extra `HEADER_GAP` guarantees eight bytes below the payload
            // for the stored padding, so recovery reads a same-position slot
            // for every alignment.
            let payload = align_up(block_addr + HEADER_SIZE + HEADER_GAP, align);
            let Some(need_end) = payload.checked_add(size) else {
                current = node.next;
                continue;
            };
            if need_end > block_addr + block_size {
                current = node.next;
                continue;
            }
            let head_pad = payload - (block_addr + HEADER_SIZE);
            let block_end = block_addr + block_size;
            let tail = block_end - need_end;
            // The leftover block start is rounded up to 8 so every header
            // stays 8-byte aligned under the aarch64 strict-align target.
            let split = tail >= MIN_BLOCK && block_end - align_up(need_end, 8) >= MIN_BLOCK;
            if split {
                let tail_addr = align_up(need_end, 8);
                let tail_size = block_end - tail_addr;
                // SAFETY: both `current` (shrunk but kept) and the new tail
                // header lie inside the live heap region; the link rewiring
                // lets the tail take over `current`'s list position.
                unsafe {
                    let tail_hdr = tail_addr as *mut Header;
                    (*tail_hdr).size = tail_size;
                    (*tail_hdr).next = (*current).next;
                    (*tail_hdr).prev = (*current).prev;
                    if !(*current).prev.is_null() {
                        (*(*current).prev).next = tail_hdr;
                    } else {
                        self.free = tail_hdr;
                    }
                    if !(*current).next.is_null() {
                        (*(*current).next).prev = tail_hdr;
                    }
                    (*current).size = tail_addr - block_addr;
                }
            } else {
                // SAFETY: unlinks `current`; the links it reads/writes are
                // this list's own slots inside the heap region.
                unsafe {
                    if !(*current).prev.is_null() {
                        (*(*current).prev).next = (*current).next;
                    } else {
                        self.free = (*current).next;
                    }
                    if !(*current).next.is_null() {
                        (*(*current).next).prev = (*current).prev;
                    }
                }
            }
            // SAFETY: `payload` lies inside the block, and the slot below the
            // payload is reserved free space large enough for the padding.
            unsafe {
                *((payload - HEADER_GAP) as *mut usize) = head_pad;
            }
            return payload as *mut u8;
        }
        core::ptr::null_mut()
    }

    /// Returns a block to the free list, coalescing it with any adjacent free
    /// neighbor.
    fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }
        let payload = ptr as usize;
        if payload < self.start + HEADER_SIZE || payload >= self.end {
            return;
        }
        // Brackets to the header through the stored padding.
        // SAFETY: `payload` came from `alloc`, which always keeps a same-size
        // `usize` slot `HEADER_GAP` bytes below the payload holding the
        // padding; reading it recovers the block header.
        let offset = unsafe { *((payload - HEADER_GAP) as *const usize) };
        let block_addr = payload - HEADER_SIZE - offset;
        if block_addr < self.start || block_addr >= self.end {
            return;
        }
        let block = block_addr as *mut Header;
        // SAFETY: `block` is a live allocation header; its size field states
        // the block extent used for coalescing.
        let block_end = unsafe { block_addr + (*block).size };
        let mut left = core::ptr::null_mut();
        let mut right = core::ptr::null_mut();
        let mut current = self.free;
        while !current.is_null() {
            // SAFETY: free-list walk, same node validity as in `alloc`.
            let node = unsafe { &*current };
            let node_addr = current as usize;
            if node_addr + node.size == block_addr {
                left = current;
            }
            if node_addr == block_end {
                right = current;
            }
            current = node.next;
        }
        if !right.is_null() {
            // SAFETY: `right` is a free block ending exactly where this block
            // ends; merging it and unlinking it keeps extents consistent.
            unsafe {
                (*block).size += (*right).size;
                if !(*right).prev.is_null() {
                    (*(*right).prev).next = (*right).next;
                } else {
                    self.free = (*right).next;
                }
                if !(*right).next.is_null() {
                    (*(*right).next).prev = (*right).prev;
                }
            }
        }
        if !left.is_null() {
            // SAFETY: `left` ends exactly where this block starts, so growing
            // it folds the freed block back into the list.
            unsafe {
                (*left).size += (*block).size;
            }
            return;
        }
        // SAFETY: prepends the freed (right-merged) block; the node slots it
        // writes lie inside the heap region.
        unsafe {
            (*block).next = self.free;
            (*block).prev = core::ptr::null_mut();
            if !self.free.is_null() {
                (*self.free).prev = block;
            }
            self.free = block;
        }
    }
}

/// Sum of every free block's size; lets host tests assert the heap never gains
/// or loses bytes to bookkeeping bugs.
#[cfg(test)]
impl Heap {
    fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut current = self.free;
        while !current.is_null() {
            // SAFETY: free-list walk, same node validity as in `alloc`.
            let node = unsafe { &*current };
            total += node.size;
            current = node.next;
        }
        total
    }
}

/// The one allocator: `Spinlock<Heap>` is itself the `#[global_allocator]`, so
/// every request takes the lock exactly once.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
// SAFETY: both methods funnel into `Heap` while the Spinlock is held, and
// `Heap` keeps every region and pointer it returns internal to `start..end`.
unsafe impl GlobalAlloc for Spinlock<Heap> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.lock().dealloc(ptr, layout);
    }
}

/// The bare-heap interior is raw-pointer state, but every access is serialized
/// by the owning `Spinlock`, so sharing the `static` across threads is sound.
// SAFETY: Heap's only interior state is the free-list head and bounds, all
// mutated exclusively while the wrapping Spinlock is held; the allocator is a
// single never-moved static, so giving the marker is what lets `Spinlock<Heap>`
// implement the `Sync` required by `GlobalAlloc`.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe impl Send for Heap {}

#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
#[global_allocator]
static ALLOCATOR: Spinlock<Heap> = Spinlock::new(Heap::new());

/// Crash path for allocation failure, routed to the kernel panic handler.
#[cfg(all(
    not(test),
    target_os = "none",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    panic!("heap exhausted requesting {layout:?}")
}

/// Fallback heap backing when the memory map reports no usable region; lives
/// in `.bss`, which the loader zeroes before handoff.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
const STATIC_ARENA_SIZE: usize = 1024 * 1024;

#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
struct StaticArena {
    // SAFETY: the array is only touched while the global heap Spinlock is
    // held (heap init plus every alloc/dealloc), never concurrently.
    data: UnsafeCell<[u8; STATIC_ARENA_SIZE]>,
}

// SAFETY: sole access is under `ALLOCATOR`'s Spinlock, so one static may be
// shared across threads.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe impl Sync for StaticArena {}

#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
static STATIC_ARENA: StaticArena = StaticArena {
    data: UnsafeCell::new([0u8; STATIC_ARENA_SIZE]),
};

/// Installs the kernel heap from the largest USABLE memory-map region
/// (virtual address = physical + HHDM offset), falling back to the static
/// arena when the loader offers none usable.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn install_from_boot_info(info: &crate::limine::BootInfo) {
    let hhdm = info.hhdm_offset as usize;
    let mut best: Option<(usize, usize)> = None;
    for entry in info.memmap.entries() {
        if entry.entry_type == crate::limine::memmap_type::USABLE
            && entry.length as usize >= MIN_BLOCK
        {
            let len = entry.length as usize;
            if best.is_none_or(|(_, best_len)| len > best_len) {
                best = Some((entry.base as usize + hhdm, len));
            }
        }
    }
    if let Some((start, len)) = best
        && apply_region(start, len)
    {
        return;
    }
    // Guarded by the same Spinlock taken in `apply_region`; the arena is only
    // touched by the heap. Alignment is handled inside init.
    let fallback = STATIC_ARENA.data.get() as usize;
    apply_region(fallback, STATIC_ARENA_SIZE);
}

#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
fn apply_region(start: usize, size: usize) -> bool {
    ALLOCATOR.lock().init(start, size)
}

/// Allocates a `Box`, `Vec`, and `Rc`, fills and reads each, then drops them —
/// proves the heap on the boot path before the UI ever uses `alloc`.
#[cfg(all(not(test), any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn heap_smoke() {
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::vec::Vec;

    let mut vec = Vec::with_capacity(64);
    for i in 0..64 {
        vec.push(i as u64);
    }
    assert_eq!(vec.iter().sum::<u64>(), 63 * 64 / 2);

    let boxed = Box::new([0xabu8; 256]);
    assert!(boxed.iter().all(|&b| b == 0xab));

    let shared = Rc::new(0x4242u32);
    assert_eq!(*shared, 0x4242);
    drop(vec);
    drop(boxed);
    drop(shared);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Fresh `Heap` over an owned 8-aligned region that stays put for the test.
    fn heap_pair(bytes: usize) -> (Spinlock<Heap>, Vec<u64>) {
        let words = bytes.div_ceil(8).max(1);
        let backing = alloc::vec![0u64; words];
        let heap = Spinlock::new(Heap::new());
        let ok = heap
            .lock()
            .init(backing.as_ptr() as usize, backing.len() * 8);
        assert!(ok, "test region must be large enough");
        (heap, backing)
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).expect("valid test layout")
    }

    /// Recovers a live block's `[payload_end, header_start, header_end)` the
    /// same way `dealloc` does: the stored gap at `payload - HEADER_GAP` plus
    /// the size field of the header the gap points at.
    fn block_bounds(payload: usize) -> (usize, usize, usize) {
        // SAFETY: mirrors the allocator's own recovery contract: the slot is
        // always written by `alloc` below an outstanding payload.
        let offset = unsafe { *((payload - HEADER_GAP) as *const usize) };
        let block_addr = payload - HEADER_SIZE - offset;
        // SAFETY: `block_addr` is this block's header, in-bounds and written
        // by the allocator.
        let block_end = unsafe { block_addr + (*(block_addr as *const Header)).size };
        (payload, block_addr, block_end)
    }

    /// Walks the free list asserting every node is in-region, non-empty,
    /// 8-aligned, and that no two nodes overlap; an acyclic-bounds check also
    /// catches next-pointers pointing into garbage.
    #[track_caller]
    fn assert_free_list_shape(heap: &Spinlock<Heap>) {
        let start = heap.lock().start;
        let end = heap.lock().end;
        let mut nodes: Vec<(usize, usize)> = Vec::new();
        let mut cursor = heap.lock().free;
        let mut strides = 0usize;
        while !cursor.is_null() {
            strides += 1;
            assert!(strides < 1 << 20, "free list is cyclic");
            // SAFETY: test-only walk, same node validity rules as the heap.
            let h = unsafe { &*cursor };
            let addr = cursor as usize;
            assert!(
                h.size >= HEADER_SIZE + HEADER_GAP,
                "node size {} at {addr:#x}",
                h.size
            );
            assert!(addr.is_multiple_of(8), "node {addr:#x} not 8-aligned");
            assert!(
                addr >= start && addr + h.size <= end,
                "node [{addr:#x}, {:#x}) outside heap [{start:#x}, {end:#x})",
                addr + h.size
            );
            nodes.push((addr, h.size));
            cursor = h.next;
        }
        nodes.sort_unstable_by_key(|&(addr, _)| addr);
        for pair in nodes.windows(2) {
            let (a_addr, a_size) = pair[0];
            let (b_addr, _) = pair[1];
            assert!(
                a_addr + a_size <= b_addr,
                "free nodes [{a_addr:#x}, {:#x}) and [{b_addr:#x}, ...) overlap",
                a_addr + a_size
            );
        }
    }

    #[test]
    fn init_rejects_tiny_or_misaligned_regions() {
        let (heap, backing) = heap_pair(MIN_BLOCK * 2);
        assert!(!heap.lock().init(backing.as_ptr() as usize, MIN_BLOCK - 1));
        assert!(!heap.lock().init(usize::MAX - 7, MIN_BLOCK));
    }

    #[test]
    fn first_fit_reuses_an_earlier_freed_block() {
        let (heap, _backing) = heap_pair(4096);
        let a = heap.lock().alloc(layout(8, 8));
        assert!(!a.is_null());
        let b = heap.lock().alloc(layout(8, 8));
        assert!(!b.is_null());
        assert_ne!(a, b);

        heap.lock().dealloc(a, layout(8, 8));
        let c = heap.lock().alloc(layout(8, 8));
        assert_eq!(c, a, "first-fit must return the head free block");
        assert_ne!(c, b);
    }

    #[test]
    fn alignment_is_honored() {
        let (heap, _backing) = heap_pair(1024 * 1024);
        for align in [1usize, 2, 4, 8, 16, 64, 256, 4096] {
            let p = heap.lock().alloc(layout(align, align));
            assert!(!p.is_null(), "align {align}");
            assert_eq!(p as usize % align, 0, "align {align}");
            heap.lock().dealloc(p, layout(align, align));
            assert_eq!(
                heap.lock().free_bytes(),
                1024 * 1024,
                "align {align} must return every byte"
            );
        }
    }

    #[test]
    fn adjacent_frees_coalesce_into_one_block() {
        let (heap, _backing) = heap_pair(4096);
        let a = heap.lock().alloc(layout(8, 8));
        let b = heap.lock().alloc(layout(8, 8));
        assert!(!a.is_null() && !b.is_null());

        heap.lock().dealloc(a, layout(8, 8));
        heap.lock().dealloc(b, layout(8, 8));

        // A 0x50 payload needs the two 0x28 blocks merged.
        let big = heap.lock().alloc(layout(0x40, 8));
        assert!(
            !big.is_null(),
            "coalesced blocks must satisfy a big request"
        );
        heap.lock().dealloc(big, layout(0x40, 8));
        assert_eq!(heap.lock().free_bytes(), 4096, "all space must be back");
    }

    #[test]
    fn exhaustion_returns_null_and_the_heap_heals() {
        let (heap, _backing) = heap_pair(MIN_BLOCK * 2);
        let a = heap.lock().alloc(layout(0x40, 8));
        assert!(!a.is_null());
        let b = heap.lock().alloc(layout(0x40, 8));
        assert!(
            b.is_null(),
            "exhausted heap must yield null (OOM abort trigger)"
        );

        heap.lock().dealloc(a, layout(0x40, 8));
        let c = heap.lock().alloc(layout(0x40, 8));
        assert_eq!(c, a, "freed block must satisfy the same request again");
    }

    #[test]
    fn zero_sized_allocations_are_distinct_and_reusable() {
        let (heap, _backing) = heap_pair(4096);
        let z = layout(0, 8);
        let a = heap.lock().alloc(z);
        let b = heap.lock().alloc(z);
        assert!(!a.is_null() && !b.is_null());
        assert_ne!(a, b);
        assert!((a as usize).is_multiple_of(8) && (b as usize).is_multiple_of(8));

        heap.lock().dealloc(a, z);
        heap.lock().dealloc(b, z);
        assert_eq!(heap.lock().free_bytes(), 4096);
    }

    #[test]
    fn free_list_never_gains_or_loses_bytes_under_random_workload() {
        let (heap, _backing) = heap_pair(1 << 20);
        const REGION: usize = 1 << 20;
        const OPS: usize = 2000;
        // LIVE tracks (payload, payload_end, block_addr, block_end) for every
        // outstanding allocation; reading a header's offset/size yields the
        // block extent, letting the test audit the allocator's own invariants.
        let mut live: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut seed = 0x1234_5678u64;

        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for _ in 0..OPS {
            // Re-read every live block's header; a misbehaving dealloc would
            // corrupt one of these between iterations.
            for &(payload, _, expect_begin, expect_end) in &live {
                let (_, begin, end) = block_bounds(payload);
                assert_eq!(
                    (begin, end),
                    (expect_begin, expect_end),
                    "live block header corrupted by a previous dealloc"
                );
            }
            if (next() & 1) == 0 || live.is_empty() {
                let size = (next() as usize % 512) + 1;
                let align = 1usize << (next() % 9);
                let l = layout(size, align);
                let p = heap.lock().alloc(l);
                if !p.is_null() {
                    let payload = p as usize;
                    assert_eq!(payload % align, 0);
                    let (payload, block_addr, block_end) = block_bounds(payload);
                    assert!(
                        block_addr < block_end,
                        "payload 0x{payload:x} -> empty block [{block_addr:#x}, {block_end:#x})"
                    );
                    for &(_, _, other_begin, other_end) in &live {
                        let separate = block_end <= other_begin || other_end <= block_addr;
                        assert!(separate, "blocks must never overlap");
                    }
                    live.push((payload, payload + l.size(), block_addr, block_end));
                }
                assert_free_list_shape(&heap);
            } else {
                let i = (next() as usize) % live.len();
                let (payload, payload_end, _, _) = live.swap_remove(i);
                assert!(payload < payload_end);
                let l = layout(payload_end - payload, 1);
                heap.lock().dealloc(payload as *mut u8, l);
                assert_free_list_shape(&heap);
            }
        }
        for (payload, payload_end, _, _) in live {
            let l = layout(payload_end - payload, 1);
            heap.lock().dealloc(payload as *mut u8, l);
        }
        assert_eq!(heap.lock().free_bytes(), REGION, "bytes must be conserved");
    }
}
