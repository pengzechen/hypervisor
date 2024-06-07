mod boot;

#[cfg(feature = "smp")]
#[cfg(not(platform_family = "aarch64-raspi"))]
pub mod mp;

#[cfg(not(platform_family = "aarch64-raspi"))]
pub mod psci;

//xh not sure
cfg_if::cfg_if! {
    if #[cfg(all(feature = "irq" , not(feature = "gic_v3")))] {
        pub mod gic;
        pub mod irq {
            pub use super::gic::*;
        }
    }

    else if #[cfg(all(feature = "irq", feature = "gic_v3"))] {
        pub mod gicv3;
        pub mod irq {
            pub use super::gicv3::*;
        }
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "hv")] {
        mod generic_timer_hv;
        pub mod time {
            pub use super::generic_timer_hv::*;
        }
    } else {
        mod generic_timer;
        pub mod time {
            pub use super::generic_timer::*;
        }
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(platform_family = "aarch64-bsta1000b", platform_family= "aarch64-rk3588j"))] {
        mod dw_apb_uart;
        pub mod console {
            pub use super::dw_apb_uart::*;
        }
    } else if #[cfg(any(platform_family = "aarch64-raspi", platform_family = "aarch64-qemu-virt"))] {
        mod pl011;
        pub mod console {
            pub use super::pl011::*;
        }
    }
}

pub mod mem;

extern "C" {
    fn exception_vector_base();
    fn rust_main(cpu_id: usize, dtb: usize);
    #[cfg(feature = "smp")]
    fn rust_main_secondary(cpu_id: usize);
}

/// The earliest entry point for the secondary CPUs.
pub(crate) unsafe extern "C" fn rust_entry(cpu_id: usize, dtb: usize) {
    //console::putchar(b'x');
    use crate::mem_map::phys_to_virt;
    crate::mem_map::clear_bss();
    crate::arch::set_exception_vector_base(exception_vector_base as usize);
    crate::cpu::init_primary(cpu_id);

    // init fdt
    crate::platform::aarch64_common::mem::idmap_device(dtb);
    of::init_fdt_ptr(phys_to_virt(dtb.into()).as_usize() as *const u8);
    // HugeMap all device memory for allocator
    for m in of::memory_nodes() {
        for r in m.regions() {
            crate::platform::aarch64_common::mem::idmap_device(r.starting_address as usize);
        }
    }
    
    console::init_early();
    time::init_early();
    // disable low address access
    crate::arch::write_page_table_root0(0.into());
    console::putchar(b'h');
    
    rust_main(cpu_id, dtb);
}

#[cfg(feature = "smp")]
pub(crate) unsafe extern "C" fn rust_entry_secondary(cpu_id: usize) {
    crate::arch::set_exception_vector_base(exception_vector_base as usize);
    crate::arch::write_page_table_root0(0.into()); // disable low address access
    crate::cpu::init_secondary(cpu_id);
    rust_main_secondary(cpu_id);
}

/// Initializes the platform devices for the primary CPU.
///
/// For example, the interrupt controller and the timer.
pub fn platform_init() {
    #[cfg(feature = "irq")]    crate::platform::gicv3::init_primary();
    
    info!("gic global local init ok!!");
    use crate::platform::aarch64_common::generic_timer_hv::init_percpu;
    init_percpu();
    
    info!("timer init ok!!");
    
    #[cfg(feature = "irq")]   console::init();
    
    info!("pl011 init ok!!");
}

/// Initializes the platform devices for secondary CPUs.
#[cfg(feature = "smp")]
pub fn platform_init_secondary() {
    #[cfg(feature = "irq")]
    crate::platform::irq::init_secondary();
    crate::platform::time::init_percpu();
}

/// Returns the name of the platform.
pub fn platform_name() -> &'static str {
    of::machin_name()
}
