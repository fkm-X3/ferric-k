//! Hand-written Limine Boot Protocol ABI.

use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

/// `LIMINE_COMMON_MAGIC`.
const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// `LIMINE_REQUESTS_START_MARKER`.
const REQUESTS_START_MARKER: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

/// `LIMINE_REQUESTS_END_MARKER`.
const REQUESTS_END_MARKER: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

/// `LIMINE_BASE_REVISION(N)` tag magic.
const BASE_REVISION_TAG_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// Base revision Ferric-K requests.
pub const REQUESTED_BASE_REVISION: u64 = 4;

/// `struct limine_<feature>_request`: `id[4]`, `revision`, `response` (48 bytes).
#[repr(C)]
pub struct Request<R> {
    id: [u64; 4],
    revision: u64,
    response: AtomicPtr<R>,
}

impl<R> Request<R> {
    pub(crate) const fn new(id: [u64; 4]) -> Self {
        Self {
            id,
            revision: 0,
            response: AtomicPtr::new(core::ptr::null_mut()),
        }
    }
}

impl<R> Request<R> {
    /// Loader-published response, if the feature was provided; absence is
    /// legitimate per-feature.
    pub fn response(&self) -> Option<&'static R> {
        let ptr = self.response.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }
        // SAFETY: non-NULL response pointers are published by the loader
        // before handoff and point into bootloader-reclaimable memory, which
        // stays mapped (via HHDM) for as long as the kernel runs because
        // Ferric-K never reclaims that memory. The pointee outlives every
        // 'static borrow we hand out here.
        Some(unsafe { &*ptr })
    }
}

const fn feature_id(a: u64, b: u64) -> [u64; 4] {
    [COMMON_MAGIC[0], COMMON_MAGIC[1], a, b]
}

/// `LIMINE_HHDM_REQUEST_ID`.
const HHDM_REQUEST_ID: [u64; 4] = feature_id(0x48dcf1cb8ad2b852, 0x63984e959a98244b);

/// `struct limine_hhdm_response`.
#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    /// Virtual offset of the direct map: virtual − offset = physical.
    pub offset: u64,
}

/// `LIMINE_FRAMEBUFFER_REQUEST_ID`.
const FRAMEBUFFER_REQUEST_ID: [u64; 4] = feature_id(0x9d5827dcd881dd75, 0xa3148604f6fab11b);

/// `LIMINE_FRAMEBUFFER_RGB` — linear RGB `memory_model`.
pub const MEMORY_MODEL_RGB: u8 = 1;

/// `struct limine_framebuffer_response`.
#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    framebuffers: *mut *mut Framebuffer,
}

impl FramebufferResponse {
    /// All framebuffers the loader gave us.
    pub fn framebuffers(&self) -> &[&'static Framebuffer] {
        // SAFETY: `framebuffers` points at an array of exactly
        // `framebuffer_count` pointers published before handoff. The protocol
        // guarantees response pointer fields (and array elements) are
        // non-NULL unless stated otherwise, and each element stays valid for
        // the kernel's lifetime (bootloader-reclaimable memory, never
        // reclaimed by us). Pointers are 8 bytes, matching `&Framebuffer`.
        unsafe {
            core::slice::from_raw_parts(
                self.framebuffers.cast_const().cast::<&Framebuffer>(),
                self.framebuffer_count as usize,
            )
        }
    }
}

/// `struct limine_framebuffer`; the tail (`mode_count`/`modes`) exists only
/// for response revision >= 1.
#[repr(C)]
pub struct Framebuffer {
    address: *mut core::ffi::c_void,
    pub width: u64,
    pub height: u64,
    /// Bytes per scanline (>= width * bpp / 8).
    pub pitch: u64,
    pub bpp: u16,
    /// See [`MEMORY_MODEL_RGB`].
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    _unused: [u8; 7],
    /// EDID length at `edid`, or 0.
    pub edid_size: u64,
    edid: *mut core::ffi::c_void,
    pub mode_count: u64,
    modes: *mut *mut VideoMode,
}

impl Framebuffer {
    /// Byte address of the framebuffer surface.
    pub fn address(&self) -> *mut u8 {
        self.address.cast::<u8>()
    }

    pub fn is_rgb(&self) -> bool {
        self.memory_model == MEMORY_MODEL_RGB
    }

    /// The EDID blob, if one was provided.
    pub fn edid(&self) -> Option<&'static [u8]> {
        if self.edid.is_null() || self.edid_size == 0 {
            return None;
        }
        // SAFETY: same lifetime argument as `Request::response`: the blob
        // lives in bootloader-reclaimable memory that stays mapped forever
        // from our point of view. Length comes straight from the loader.
        Some(unsafe {
            core::slice::from_raw_parts(self.edid.cast::<u8>(), self.edid_size as usize)
        })
    }
}

/// `struct limine_video_mode`.
#[repr(C)]
pub struct VideoMode {
    pub pitch: u64,
    pub width: u64,
    pub height: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

/// `LIMINE_MEMMAP_REQUEST_ID`.
const MEMMAP_REQUEST_ID: [u64; 4] = feature_id(0x67cf3d9d378a806f, 0xe304acdfc50c3c62);

/// `LIMINE_MEMMAP_*` region types, stored verbatim in [`MemmapEntry::entry_type`].
pub mod memmap_type {
    pub const USABLE: u64 = 0;
    pub const RESERVED: u64 = 1;
    pub const ACPI_RECLAIMABLE: u64 = 2;
    pub const ACPI_NVS: u64 = 3;
    pub const BAD_MEMORY: u64 = 4;
    pub const BOOTLOADER_RECLAIMABLE: u64 = 5;
    pub const EXECUTABLE_AND_MODULES: u64 = 6;
    pub const FRAMEBUFFER: u64 = 7;
    pub const RESERVED_MAPPED: u64 = 8;
}

/// `struct limine_memmap_response`.
#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    entries: *mut *mut MemmapEntry,
}

impl MemmapResponse {
    /// All memory map entries.
    pub fn entries(&self) -> &[&'static MemmapEntry] {
        // SAFETY: identical reasoning to `FramebufferResponse::framebuffers`:
        // loader-published array of non-NULL pointers, valid forever from our
        // side, 8-byte pointers matching references.
        unsafe {
            core::slice::from_raw_parts(
                self.entries.cast_const().cast::<&MemmapEntry>(),
                self.entry_count as usize,
            )
        }
    }
}

/// `struct limine_memmap_entry`.
#[repr(C)]
pub struct MemmapEntry {
    /// Physical start address.
    pub base: u64,
    /// Length in bytes.
    pub length: u64,
    /// `type` in the C struct ("type" is reserved in Rust).
    pub entry_type: u64,
}

/// Start delimiter, 32 bytes, 8-byte aligned per protocol.
#[repr(C, align(8))]
struct RequestsStartMarker([u64; 4]);

/// End delimiter, 16 bytes, 8-byte aligned per protocol.
#[repr(C, align(8))]
struct RequestsEndMarker([u64; 2]);

/// `LIMINE_BASE_REVISION(N)` tag (24 bytes, 8-byte aligned); atomics because
/// the loader writes before handoff: `requested_revision` becomes 0 iff the
/// requested base revision was accepted, and `magic1` is rewritten with the
/// revision actually used when it differs from the request.
#[repr(C, align(8))]
struct BaseRevisionTag {
    magic0: AtomicU64,
    magic1: AtomicU64,
    requested_revision: AtomicU64,
}

/// The loader did not accept the requested base revision.
pub struct BaseRevisionNotAccepted;

/// Negotiated protocol base revision, or [`BaseRevisionNotAccepted`].
pub fn base_revision() -> Result<u64, BaseRevisionNotAccepted> {
    if BASE_REVISION.requested_revision.load(Ordering::Acquire) != 0 {
        return Err(BaseRevisionNotAccepted);
    }
    let magic1 = BASE_REVISION.magic1.load(Ordering::Acquire);
    if magic1 == BASE_REVISION_TAG_MAGIC[1] {
        Ok(REQUESTED_BASE_REVISION) // untouched => booted at requested revision
    } else {
        Ok(magic1) // rewritten => holds the revision actually used
    }
}

#[used]
#[unsafe(link_section = ".limine_requests.start")]
static START_MARKER: RequestsStartMarker = RequestsStartMarker(REQUESTS_START_MARKER);

#[used]
#[unsafe(link_section = ".limine_requests")]
static BASE_REVISION: BaseRevisionTag = BaseRevisionTag {
    magic0: AtomicU64::new(BASE_REVISION_TAG_MAGIC[0]),
    magic1: AtomicU64::new(BASE_REVISION_TAG_MAGIC[1]),
    requested_revision: AtomicU64::new(REQUESTED_BASE_REVISION),
};

#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM_REQUEST: Request<HhdmResponse> = Request::new(HHDM_REQUEST_ID);

#[used]
#[unsafe(link_section = ".limine_requests")]
static FRAMEBUFFER_REQUEST: Request<FramebufferResponse> = Request::new(FRAMEBUFFER_REQUEST_ID);

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMMAP_REQUEST: Request<MemmapResponse> = Request::new(MEMMAP_REQUEST_ID);

#[used]
#[unsafe(link_section = ".limine_requests.end")]
static END_MARKER: RequestsEndMarker = RequestsEndMarker(REQUESTS_END_MARKER);

/// Validated snapshot of everything the bootloader handed us.
pub struct BootInfo {
    pub base_revision: u64,
    pub hhdm_offset: u64,
    pub memmap: &'static MemmapResponse,
    /// Empty if none were provided.
    pub framebuffers: &'static [&'static Framebuffer],
}

/// Collects and validates the boot-time feature responses; `None` when the
/// environment misses minimum requirements (base revision rejected, HHDM or
/// memory map unanswered). The framebuffer is optional.
pub(crate) fn collect() -> Option<BootInfo> {
    let base_revision = match base_revision() {
        Ok(rev) => rev,
        Err(BaseRevisionNotAccepted) => return None,
    };
    let hhdm = HHDM_REQUEST.response()?;
    let memmap = MEMMAP_REQUEST.response()?;
    let framebuffers: &'static [&'static Framebuffer] = match FRAMEBUFFER_REQUEST.response() {
        Some(resp) => resp.framebuffers(),
        None => &[],
    };
    Some(BootInfo {
        base_revision,
        hhdm_offset: hhdm.offset,
        memmap,
        framebuffers,
    })
}

#[cfg(test)]
mod abi_layout_tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn request_header_is_48_bytes_with_documented_offsets() {
        assert_eq!(size_of::<Request<HhdmResponse>>(), 48);
        assert_eq!(offset_of!(Request<HhdmResponse>, id), 0x00);
        assert_eq!(offset_of!(Request<HhdmResponse>, revision), 0x20);
        assert_eq!(offset_of!(Request<HhdmResponse>, response), 0x28);
    }

    #[test]
    fn marker_sizes_match_specification() {
        assert_eq!(size_of::<RequestsStartMarker>(), 32);
        assert_eq!(size_of::<RequestsEndMarker>(), 16);
        assert_eq!(size_of::<BaseRevisionTag>(), 24);
    }

    #[test]
    fn hhdm_response_layout() {
        assert_eq!(size_of::<HhdmResponse>(), 16);
        assert_eq!(offset_of!(HhdmResponse, revision), 0x00);
        assert_eq!(offset_of!(HhdmResponse, offset), 0x08);
    }

    #[test]
    fn framebuffer_struct_layout_matches_c_offsets() {
        assert_eq!(size_of::<Framebuffer>(), 80);
        assert_eq!(offset_of!(Framebuffer, address), 0x00);
        assert_eq!(offset_of!(Framebuffer, width), 0x08);
        assert_eq!(offset_of!(Framebuffer, height), 0x10);
        assert_eq!(offset_of!(Framebuffer, pitch), 0x18);
        assert_eq!(offset_of!(Framebuffer, bpp), 0x20);
        assert_eq!(offset_of!(Framebuffer, memory_model), 0x22);
        assert_eq!(offset_of!(Framebuffer, red_mask_size), 0x23);
        assert_eq!(offset_of!(Framebuffer, green_mask_shift), 0x26);
        assert_eq!(offset_of!(Framebuffer, blue_mask_shift), 0x28);
        assert_eq!(offset_of!(Framebuffer, edid_size), 0x30);
        assert_eq!(offset_of!(Framebuffer, edid), 0x38);
        assert_eq!(offset_of!(Framebuffer, mode_count), 0x40);
        assert_eq!(offset_of!(Framebuffer, modes), 0x48);
    }

    #[test]
    fn video_mode_and_response_layouts() {
        assert_eq!(size_of::<VideoMode>(), 40);
        assert_eq!(size_of::<FramebufferResponse>(), 24);
        assert_eq!(offset_of!(FramebufferResponse, framebuffer_count), 0x08);
        assert_eq!(offset_of!(FramebufferResponse, framebuffers), 0x10);
    }

    #[test]
    fn memmap_layouts() {
        assert_eq!(size_of::<MemmapEntry>(), 24);
        assert_eq!(size_of::<MemmapResponse>(), 24);
        assert_eq!(offset_of!(MemmapEntry, base), 0x00);
        assert_eq!(offset_of!(MemmapEntry, length), 0x08);
        assert_eq!(offset_of!(MemmapEntry, entry_type), 0x10);
    }

    #[test]
    fn request_ids_are_distinct_and_magic_prefixed() {
        let ids = [HHDM_REQUEST_ID, FRAMEBUFFER_REQUEST_ID, MEMMAP_REQUEST_ID];
        for id in ids {
            assert_eq!(&id[..2], &COMMON_MAGIC);
        }
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(&ids[i][2..], &ids[j][2..]);
            }
        }
    }
}
