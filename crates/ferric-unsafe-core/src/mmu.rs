//! Minimal AArch64 stage-1 page-descriptor logic for extending the
//! bootloader's live higher-half tables with MMIO windows: the bootloader's
//! direct map covers memory-map ranges only, so device regions are unmapped
//! at handoff. Descriptor bit positions per ARM DDI 0487 stage-1 format,
//! matching Limine v12.6.0 `vmm.c`.

use crate::volatile::Volatile;

pub const ENTRIES: usize = 512;
pub const BLOCK_2MIB: u64 = 0x20_0000;

// Stage-1 descriptor bits (ARM DDI 0487); block/page distinction is bit 1.
pub const VALID: u64 = 1 << 0;
pub const TABLE_OR_PAGE: u64 = 1 << 1;
pub const ATTR_IDX_WB: u64 = 0b000 << 2;
pub const AP_EL1_RW: u64 = 0b00 << 6;
pub const INNER_SHAREABLE: u64 = 0b11 << 8;
pub const ACCESSED: u64 = 1 << 10;
pub const UXN: u64 = 1 << 54;

/// Output-address field shared by table/block/page descriptors.
pub const OUT_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

pub const fn l0_index(va: u64) -> usize {
    ((va >> 39) & 0x1ff) as usize
}

pub const fn l1_index(va: u64) -> usize {
    ((va >> 30) & 0x1ff) as usize
}

/// One 2 MiB block descriptor: EL1-only RW, never executable, normal-WB via
/// MAIR attribute 0, inner-shareable, access flag pre-set.
pub const fn block_descriptor_2m(phys_base: u64) -> u64 {
    debug_assert!(phys_base.is_multiple_of(BLOCK_2MIB));
    phys_base | VALID | ATTR_IDX_WB | AP_EL1_RW | INNER_SHAREABLE | ACCESSED | UXN
}

/// Why a 2 MiB block could not be inserted under an existing top-level entry.
#[derive(Debug, PartialEq, Eq)]
pub enum InsertBlockError {
    /// The referenced top-level entry is not marked valid.
    TopInvalid,
    /// The referenced top-level entry points to a block, not a table.
    TopNotTable,
    /// The target L1 slot already holds a descriptor.
    Occupied,
    /// The block base is not 2 MiB aligned.
    Misaligned,
}

/// Inserts `block_descriptor_2m(phys_base)` at the L1 slot covering `va`,
/// validating but never mutating the top-level table. `l1` must be the table
/// the (validated) top-level descriptor points at.
///
/// Both tables are addressed through [`Volatile`] because they are shared,
/// bootloader-owned storage written ahead of a TLB flush.
pub fn insert_block_2m(
    top: &[Volatile<u64>; ENTRIES],
    l1: &mut [Volatile<u64>; ENTRIES],
    va: u64,
    phys_base: u64,
) -> Result<(), InsertBlockError> {
    if !phys_base.is_multiple_of(BLOCK_2MIB) {
        return Err(InsertBlockError::Misaligned);
    }
    let top_entry = top[l0_index(va)].read();
    if top_entry & VALID == 0 {
        return Err(InsertBlockError::TopInvalid);
    }
    if top_entry & TABLE_OR_PAGE == 0 {
        return Err(InsertBlockError::TopNotTable);
    }
    let idx = l1_index(va);
    if l1[idx].read() & VALID != 0 {
        return Err(InsertBlockError::Occupied);
    }
    l1[idx].write(block_descriptor_2m(phys_base));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handles_over(backing: &mut [u64; ENTRIES]) -> [Volatile<u64>; ENTRIES] {
        core::array::from_fn(|i| Volatile::new(&mut backing[i] as *mut u64))
    }

    #[test]
    fn qemu_virt_uart_window_indices() {
        const VA: u64 = 0xffff_0000_0900_0000;
        assert_eq!(l0_index(VA), 0);
        assert_eq!(l1_index(VA), 0);
    }

    #[test]
    fn pl011_base_is_2mib_aligned() {
        assert_eq!(crate::pl011::UART0_BASE as u64 % BLOCK_2MIB, 0);
    }

    #[test]
    fn block_descriptor_carries_el1_rw_wb_nonexec_bits() {
        let d = block_descriptor_2m(0x0900_0000);
        assert_eq!(d & VALID, VALID);
        assert_eq!(d & TABLE_OR_PAGE, 0); // block, not table/page
        assert_eq!(d & ATTR_IDX_WB, 0);
        assert_eq!(d & AP_EL1_RW, 0);
        assert_eq!(d & INNER_SHAREABLE, INNER_SHAREABLE);
        assert_eq!(d & ACCESSED, ACCESSED);
        assert_eq!(d & UXN, UXN);
        assert_eq!(d & OUT_ADDR_MASK, 0x0900_0000);
    }

    #[test]
    fn inserts_block_into_fresh_slot() {
        let mut top_backing = [0u64; ENTRIES];
        let mut l1_backing = [0u64; ENTRIES];
        let mut top = handles_over(&mut top_backing);
        let mut l1 = handles_over(&mut l1_backing);
        let va = 0xffff_0000_0900_0000;

        top[l0_index(va)].write(0x1234_5000 | VALID | TABLE_OR_PAGE);

        insert_block_2m(&top, &mut l1, va, 0x0900_0000).unwrap();
        assert_eq!(l1[l1_index(va)].read(), block_descriptor_2m(0x0900_0000));
    }

    #[test]
    fn refuses_to_touch_occupied_invalid_or_nontable_slots() {
        let va = 0xffff_0000_0900_0000;
        let idx = l1_index(va);

        let mut top_backing = [0u64; ENTRIES];
        let mut l1_backing = [0u64; ENTRIES];
        let mut top = handles_over(&mut top_backing);
        let mut l1 = handles_over(&mut l1_backing);
        top[l0_index(va)].write(0); // invalid entry
        assert_eq!(
            insert_block_2m(&top, &mut l1, va, 0x0900_0000),
            Err(InsertBlockError::TopInvalid)
        );

        let mut top_backing = [0u64; ENTRIES];
        let mut l1_backing = [0u64; ENTRIES];
        let mut top = handles_over(&mut top_backing);
        let mut l1 = handles_over(&mut l1_backing);
        top[l0_index(va)].write(0x1234_5000 | VALID); // block-shaped top entry
        assert_eq!(
            insert_block_2m(&top, &mut l1, va, 0x0900_0000),
            Err(InsertBlockError::TopNotTable)
        );

        let mut top_backing = [0u64; ENTRIES];
        let mut l1_backing = [0u64; ENTRIES];
        let mut top = handles_over(&mut top_backing);
        let mut l1 = handles_over(&mut l1_backing);
        top[l0_index(va)].write(0x1234_5000 | VALID | TABLE_OR_PAGE);
        l1[idx].write(block_descriptor_2m(0x0a00_0000));
        assert_eq!(
            insert_block_2m(&top, &mut l1, va, 0x0900_0000),
            Err(InsertBlockError::Occupied)
        );
        assert_eq!(l1[idx].read(), block_descriptor_2m(0x0a00_0000));

        let mut top_backing = [0u64; ENTRIES];
        let mut l1_backing = [0u64; ENTRIES];
        let mut top = handles_over(&mut top_backing);
        let mut l1 = handles_over(&mut l1_backing);
        top[l0_index(va)].write(0x1234_5000 | VALID | TABLE_OR_PAGE);
        assert_eq!(
            insert_block_2m(&top, &mut l1, va, 0x0910_0000),
            Err(InsertBlockError::Misaligned)
        );
    }
}
