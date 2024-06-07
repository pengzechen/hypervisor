use aarch64_cpu::{asm::barrier, registers::*};
use tock_registers::interfaces::{ReadWriteable, Writeable};

use crate::mem_map::virt_to_phys;
use crate::mem_map::{MemRegion, MemRegionFlags, PhysAddr};
use page_table_entry::aarch64::{MemAttr, A64PTE};
use page_table_entry::{GenericPTE, MappingFlags};

use either::{Either, Left, Right};

/// Returns platform-specific memory regions.
pub(crate) fn platform_regions() -> impl Iterator<Item = MemRegion> {
    // Feature, should registerd by user, should'n use hard coding
    let iterator: Either<_, _> = if of::machin_name().contains("raspi") {
        Left(
            core::iter::once(MemRegion {
                paddr: 0x0.into(),
                size: 0x1000,
                flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
                name: "spintable",
            })
            .chain(core::iter::once(fdt_region()))
            .chain(free_regions())
            .chain(crate::mem_map::default_mmio_regions()),
        )
    } else {
        Right(
            core::iter::once(fdt_region())
                .chain(free_regions())
                .chain(crate::mem_map::default_mmio_regions()),
        )
    };
    iterator.into_iter()
}

fn split_region(region: MemRegion, region2: &MemRegion) -> impl Iterator<Item = MemRegion> {
    let start1 = region.paddr.as_usize();
    let end1 = region.paddr.as_usize() + region.size;

    let start2 = region2.paddr.as_usize();
    let end2 = region2.paddr.as_usize() + region2.size;

    // mem region include region2
    let iterator: Either<_, _> = if start1 <= start2 && end1 >= end2 {
        let head_region = MemRegion {
            paddr: region.paddr,
            size: start2 - start1,
            flags: MemRegionFlags::FREE | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: region.name,
        };
        let tail_region = MemRegion {
            paddr: PhysAddr::from(end2),
            size: end1 - end2,
            flags: MemRegionFlags::FREE | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: region.name,
        };
        let size_tup = (head_region.size, tail_region.size);
        // Top(down) left size < 4K, need drop
        match size_tup {
            (x, y) if x < 0x1000 && y < 0x1000 => panic!("no vailid left region"),
            (x, _) if x < 0x1000 => Right([tail_region]),
            (_, y) if y < 0x1000 => Right([head_region]),
            _ => Left([head_region, tail_region]),
        }
    } else {
        Right([region])
    };
    iterator.into_iter()
}

// Free mem regions equal memory minus kernel and fdt region
fn free_regions() -> impl Iterator<Item = MemRegion> {
    let all_mem = of::memory_nodes().flat_map(|m| {
        m.regions().filter_map(|r| {
            if r.size.unwrap() > 0 {
                Some(MemRegion {
                    paddr: PhysAddr::from(r.starting_address as usize).align_up_4k(),
                    size: r.size.unwrap(),
                    flags: MemRegionFlags::FREE | MemRegionFlags::READ | MemRegionFlags::WRITE,
                    name: "free memory",
                })
            } else {
                None
            }
        })
    });

    let hack_k_region = MemRegion {
        paddr: virt_to_phys((stext as usize).into()).align_up_4k(),
        size: ekernel as usize - stext as usize,
        flags: MemRegionFlags::FREE,
        name: "kernel memory",
    };

    let filter_kernel_mem = all_mem.flat_map(move |m| split_region(m, &hack_k_region).into_iter());
    filter_kernel_mem.flat_map(move |m| split_region(m, &fdt_region()).into_iter())
}

const FDT_FIX_SIZE: usize = 0x10_0000; //1M
fn fdt_region() -> MemRegion {
    let fdt_ptr = of::get_fdt_ptr();
    MemRegion {
        paddr: virt_to_phys((fdt_ptr.unwrap() as usize).into()).align_up_4k(),
        size: FDT_FIX_SIZE,
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ,
        name: "fdt reserved",
    }
}

#[link_section = ".data.boot_page_table"]
pub static mut BOOT_PT_L0: [A64PTE; 512] = [A64PTE::empty(); 512];

#[link_section = ".data.boot_page_table"]
pub static mut BOOT_PT_L1: [A64PTE; 512] = [A64PTE::empty(); 512];

pub(crate) unsafe fn init_mmu() {
    MAIR_EL1.set(MemAttr::MAIR_VALUE);

    // Enable TTBR0 and TTBR1 walks, page size = 4K, vaddr size = 48 bits, paddr size = 40 bits.
    let tcr_flags0 = TCR_EL1::EPD0::EnableTTBR0Walks
        + TCR_EL1::TG0::KiB_4
        + TCR_EL1::SH0::Inner
        + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T0SZ.val(16);
    let tcr_flags1 = TCR_EL1::EPD1::EnableTTBR1Walks
        + TCR_EL1::TG1::KiB_4
        + TCR_EL1::SH1::Inner
        + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T1SZ.val(16);
    TCR_EL1.write(TCR_EL1::IPS::Bits_48 + tcr_flags0 + tcr_flags1);
    barrier::isb(barrier::SY);

    // Set both TTBR0 and TTBR1
    let root_paddr = PhysAddr::from(BOOT_PT_L0.as_ptr() as usize).as_usize() as _;
    TTBR0_EL1.set(root_paddr);
    TTBR1_EL1.set(root_paddr);

    // Flush the entire TLB
    crate::arch::flush_tlb(None);

    // Enable the MMU and turn on I-cache and D-cache
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
    barrier::isb(barrier::SY);
}

pub(crate)  unsafe fn init_mmu_el2() {
    /* 
    MAIR_EL2.write(
        MAIR_EL2::Attr0_Device::nonGathering_nonReordering_noEarlyWriteAck
            + MAIR_EL2::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL2::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL2::Attr2_Normal_Outer::NonCacheable
            + MAIR_EL2::Attr2_Normal_Inner::NonCacheable,
    );
    TCR_EL2.write(
        TCR_EL2::PS::Bits_40
            + TCR_EL2::SH0::Inner
            + TCR_EL2::TG0::KiB_4
            + TCR_EL2::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL2::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL2::T0SZ.val(16),
    );
    */
    idmap_device(0xfeb5_0000);
    // Set EL1 to 64bit.
    HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);

    // Device-nGnRE memory
    let attr0 = MAIR_EL2::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck;
    // Normal memory
    let attr1 = MAIR_EL2::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
        + MAIR_EL2::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc;
    MAIR_EL2.write(attr0 + attr1); // 0xff_04

     // Enable TTBR0 and TTBR1 walks, page size = 4K, vaddr size = 48 bits, paddr size = 40 bits.
    let tcr_flags0 = TCR_EL2::TG0::KiB_4
        + TCR_EL2::SH0::Inner
         + TCR_EL2::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
         + TCR_EL2::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
         + TCR_EL2::T0SZ.val(16);
    TCR_EL2.write(TCR_EL2::PS::Bits_40 + tcr_flags0);
    barrier::isb(barrier::SY);

    let root_paddr = PhysAddr::from(BOOT_PT_L0.as_ptr() as usize).as_usize() as _;
    TTBR0_EL2.set(root_paddr);
    // #[macro_use]
    // hypercraft::msr!(TTBR1_EL2, root_paddr);

    // Flush the entire TLB
    crate::arch::flush_tlb(None);

    // Enable the MMU and turn on I-cache and D-cache
    // SCTLR_EL2.set(0x30c51835);
    SCTLR_EL2.modify(SCTLR_EL2::M::Enable + SCTLR_EL2::C::Cacheable + SCTLR_EL2::I::Cacheable);
    barrier::isb(barrier::SY);
}

const BOOT_MAP_SHIFT: usize = 30; // 1GB
const BOOT_MAP_SIZE: usize = 1 << BOOT_MAP_SHIFT; // 1GB

 pub(crate) unsafe extern "C" fn idmap_kernel(kernel_phys_addr: usize) {
    let aligned_address = (kernel_phys_addr) & !(BOOT_MAP_SIZE - 1);
    let l1_index = kernel_phys_addr >> BOOT_MAP_SHIFT;

    // 0x0000_0000_0000 ~ 0x0080_0000_0000, table
    BOOT_PT_L0[0] = A64PTE::new_table(PhysAddr::from(BOOT_PT_L1.as_ptr() as usize));
    // 1G block, kernel img
    BOOT_PT_L1[l1_index] = A64PTE::new_page(
        PhysAddr::from(aligned_address),
        MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
        true,
    );

    //idmap_device(0x900_0000);
}

pub(crate) unsafe fn idmap_device(phys_addr: usize) {
    let aligned_address = (phys_addr) & !(BOOT_MAP_SIZE - 1);
    let l1_index = phys_addr >> BOOT_MAP_SHIFT;
    if BOOT_PT_L1[l1_index].is_unused() {
        BOOT_PT_L1[l1_index] = A64PTE::new_page(
            PhysAddr::from(aligned_address),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
            true,
        );
    }
}

extern "C" {
}


extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss();
    fn ebss();
    fn boot_stack();
    fn boot_stack_top();
    fn percpu_start();
    fn percpu_end();
    fn skernel();
    fn ekernel();

    fn sguest();
    fn eguest();

}

#[allow(dead_code)]
pub(crate) const fn common_memory_regions_num() -> usize {
    6 + axconfig::MMIO_REGIONS.len()
}

#[allow(dead_code)]
pub(crate) fn common_memory_region_at(idx: usize) -> Option<MemRegion> {
    let mmio_regions = axconfig::MMIO_REGIONS;
    let r = match idx {
        0 => MemRegion {
            paddr: virt_to_phys((stext as usize).into()),
            size: etext as usize - stext as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::EXECUTE,
            name: ".text",
        },
        1 => MemRegion {
            paddr: virt_to_phys((srodata as usize).into()),
            size: erodata as usize - srodata as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ,
            name: ".rodata",
        },
        2 => MemRegion {
            paddr: virt_to_phys((sdata as usize).into()),
            size: edata as usize - sdata as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: ".data",
        },
        3 => MemRegion {
            paddr: virt_to_phys((percpu_start as usize).into()),
            size: percpu_end as usize - percpu_start as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: ".percpu",
        },
        4 => MemRegion {
            paddr: virt_to_phys((boot_stack as usize).into()),
            size: boot_stack_top as usize - boot_stack as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: "boot stack",
        },
        5 => MemRegion {
            paddr: virt_to_phys((sbss as usize).into()),
            size: ebss as usize - sbss as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: ".bss",
        },
        6 => MemRegion {
            paddr: virt_to_phys((sguest as usize).into()),
            size: eguest as usize - sguest as usize,
            flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
            name: ".guest",
        },
        i if i < 6 + mmio_regions.len() => MemRegion {
            paddr: mmio_regions[i - 6].0.into(),
            size: mmio_regions[i - 6].1,
            flags: MemRegionFlags::RESERVED
                | MemRegionFlags::DEVICE
                | MemRegionFlags::READ
                | MemRegionFlags::WRITE,
            name: "mmio",
        },
        _ => return None,
    };
    Some(r)
}

