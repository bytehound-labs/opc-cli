#![no_std]
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

type HRESULT = i32;

const S_OK: HRESULT = 0;
const S_FALSE: HRESULT = 1;

#[no_mangle]
pub unsafe extern "system" fn RoOriginateError(_error: HRESULT, _message: *const c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn RoOriginateErrorW(_error: HRESULT, _cch_max: u32, _message: *const u16) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn RoTransformError(_old_error: HRESULT, _new_error: HRESULT, _message: *const c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn SetRestrictedErrorInfo(_restricted_error_info: *const c_void) -> HRESULT {
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn GetRestrictedErrorInfo(restricted_error_info: *mut *mut c_void) -> HRESULT {
    if !restricted_error_info.is_null() {
        *restricted_error_info = core::ptr::null_mut();
    }
    S_FALSE
}

#[no_mangle]
pub unsafe extern "system" fn RoClearError() {}

#[no_mangle]
pub unsafe extern "system" fn RoCaptureErrorContext(_hr: HRESULT) -> HRESULT {
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn RoFailFastWithErrorContext(_hr: HRESULT) {}

#[no_mangle]
pub unsafe extern "system" fn RoReportUnhandledError(_error_info: *const c_void) -> HRESULT {
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn RoGetMatchingErrorRestricted(
    _hr: HRESULT,
    _restricted_error_string: *mut *mut c_void,
    _error_info: *mut *mut c_void,
) -> HRESULT {
    S_FALSE
}
