#![no_std]
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[link(name = "kernel32")]
extern "system" {
    #[link_name = "Sleep"]
    fn Kernel32Sleep(dw_milliseconds: u32);
}

/// Re-export Sleep so the PE loader can resolve it from this API set DLL.
#[no_mangle]
pub unsafe extern "system" fn Sleep(dw_milliseconds: u32) {
    Kernel32Sleep(dw_milliseconds);
}

/// Polyfill for `WaitOnAddress` (Windows 8+).
///
/// Spins with 1ms sleeps until the value at `address` differs from
/// `compare_address`, or the timeout expires.
#[no_mangle]
pub unsafe extern "system" fn WaitOnAddress(
    address: *const c_void,
    compare_address: *const c_void,
    address_size: usize,
    milliseconds: u32,
) -> i32 {
    if address.is_null() || compare_address.is_null() {
        return 0;
    }

    let mut elapsed: u32 = 0;

    loop {
        let is_equal = match address_size {
            1 => *(address as *const u8) == *(compare_address as *const u8),
            2 => *(address as *const u16) == *(compare_address as *const u16),
            4 => *(address as *const u32) == *(compare_address as *const u32),
            8 => *(address as *const u64) == *(compare_address as *const u64),
            _ => {
                let mut eq = true;
                let a = address as *const u8;
                let b = compare_address as *const u8;
                for i in 0..address_size {
                    if *a.add(i) != *b.add(i) {
                        eq = false;
                        break;
                    }
                }
                eq
            }
        };

        if !is_equal {
            return 1;
        }

        if milliseconds != 0xFFFF_FFFF && elapsed >= milliseconds {
            return 0;
        }

        Kernel32Sleep(1);
        if milliseconds != 0xFFFF_FFFF {
            elapsed = elapsed.saturating_add(1);
        }
    }
}

/// No-op polyfill — wakes one thread waiting on `WaitOnAddress`.
#[no_mangle]
pub unsafe extern "system" fn WakeByAddressSingle(_address: *const c_void) {}

/// No-op polyfill — wakes all threads waiting on `WaitOnAddress`.
#[no_mangle]
pub unsafe extern "system" fn WakeByAddressAll(_address: *const c_void) {}
