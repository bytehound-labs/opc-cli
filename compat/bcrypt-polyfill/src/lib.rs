#![no_std]
#![allow(non_snake_case)]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[link(name = "advapi32")]
extern "system" {
    /// RtlGenRandom (a.k.a. SystemFunction036) — cryptographically secure
    /// RNG available on all Windows versions since XP SP2.
    #[link_name = "SystemFunction036"]
    fn RtlGenRandom(pb_buffer: *mut u8, cb_buffer: u32) -> u8;
}

/// Polyfill for `ProcessPrng` (Windows 8+ / bcryptprimitives.dll).
///
/// Routes random byte generation to `RtlGenRandom` in `advapi32.dll`.
#[no_mangle]
pub unsafe extern "system" fn ProcessPrng(pb_data: *mut u8, cb_data: usize) -> i32 {
    if pb_data.is_null() || cb_data == 0 {
        return 1;
    }
    let res = RtlGenRandom(pb_data, cb_data as u32);
    if res != 0 { 1 } else { 0 }
}
