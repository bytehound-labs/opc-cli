//! COM memory management and type conversion utilities for the OPC DA client.
//!
//! This module provides safe wrappers around COM memory allocations and arrays,
//! as well as traits for converting between COM-native and Rust-native types.

use windows::{
    Win32::System::{
        Com::{CoGetMalloc, CoTaskMemAlloc, CoTaskMemFree},
        Variant::{VARIANT, VariantClear, VariantCopy},
    },
    core::PWSTR,
};

type ElementCleanup<T> = unsafe fn(&mut T);
type PointerCleanup<T> = unsafe fn(*mut T);

// ── Memory Management ───────────────────────────────────────────────

/// A safe wrapper around arrays allocated by COM.
///
/// This struct ensures proper cleanup of COM-allocated memory when dropped.
/// It provides safe access to the underlying array through slices.
pub struct RemoteArray<T: Sized> {
    pointer: RemotePointer<T>,
    len: u32,
    capacity: Option<u32>,
    element_cleanup: Option<ElementCleanup<T>>,
}

impl<T: Sized> RemoteArray<T> {
    /// Creates a new `RemoteArray` with the specified length.
    /// The underlying pointer is initialized to null.
    #[inline(always)]
    pub fn new(len: u32) -> Self {
        Self {
            pointer: RemotePointer::null(),
            len,
            capacity: Some(len),
            element_cleanup: None,
        }
    }

    /// Creates a COM output array whose initialized elements require cleanup.
    #[inline(always)]
    pub(crate) fn new_with_cleanup(len: u32, cleanup: ElementCleanup<T>) -> Self {
        Self {
            pointer: RemotePointer::null(),
            len,
            capacity: Some(len),
            element_cleanup: Some(cleanup),
        }
    }

    /// Creates an empty COM output array with a known maximum returned length.
    #[inline(always)]
    pub(crate) fn with_capacity_and_cleanup(capacity: u32, cleanup: ElementCleanup<T>) -> Self {
        Self {
            pointer: RemotePointer::null(),
            len: 0,
            capacity: Some(capacity),
            element_cleanup: Some(cleanup),
        }
    }

    /// Creates a `RemoteArray` from a raw pointer and length.
    ///
    /// # Safety
    /// The caller must ensure that the pointer is valid and points to a COM-allocated array.
    #[inline(always)]
    pub(crate) unsafe fn from_mut_ptr(pointer: *mut T, len: u32) -> Self {
        Self {
            // SAFETY: The caller transfers ownership of this COM allocation.
            pointer: unsafe { RemotePointer::from_raw(pointer) },
            len,
            capacity: Some(len),
            element_cleanup: None,
        }
    }

    /// Creates an owning array from a COM allocation with nested element cleanup.
    #[inline(always)]
    pub(crate) unsafe fn from_mut_ptr_with_cleanup(
        pointer: *mut T,
        len: u32,
        cleanup: ElementCleanup<T>,
    ) -> Self {
        Self {
            // SAFETY: The caller transfers ownership of this COM allocation.
            pointer: unsafe { RemotePointer::from_raw(pointer) },
            len,
            capacity: Some(len),
            element_cleanup: Some(cleanup),
        }
    }

    /// Creates an empty `RemoteArray`.
    #[inline(always)]
    pub fn empty() -> Self {
        Self {
            pointer: RemotePointer::null(),
            len: 0,
            capacity: None,
            element_cleanup: None,
        }
    }

    /// Creates an empty COM output array whose returned elements require cleanup.
    #[inline(always)]
    pub(crate) fn empty_with_cleanup(cleanup: ElementCleanup<T>) -> Self {
        Self {
            pointer: RemotePointer::null(),
            len: 0,
            capacity: None,
            element_cleanup: Some(cleanup),
        }
    }

    /// Returns a mutable pointer to the array pointer.
    ///
    /// This is useful when calling COM functions that output an array via a pointer to a pointer.
    #[inline(always)]
    pub(crate) fn as_mut_ptr(&mut self) -> *mut *mut T {
        self.pointer.as_mut_ptr()
    }

    /// Returns a slice to the underlying array.
    ///
    /// # Safety
    /// The caller must ensure that the `pointer` is valid for reads and points to an array of `len` elements.
    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        if self.pointer.inner.is_null() || self.len == 0 {
            return &[];
        }

        let len = self.initialized_len();

        // SAFETY: Pointer and length are guaranteed to be valid for slice creation.
        unsafe { core::slice::from_raw_parts(self.pointer.inner, len) }
    }

    /// Returns a mutable slice to the underlying array.
    ///
    /// # Safety
    /// The caller must ensure that the `pointer` is valid for reads and writes and points to an array of `len` elements.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.pointer.inner.is_null() || self.len == 0 {
            return &mut [];
        }

        let len = self.initialized_len();

        // SAFETY: Pointer and length are guaranteed to be valid for mutable slice creation.
        unsafe { core::slice::from_raw_parts_mut(self.pointer.inner, len) }
    }

    /// Returns the length of the array.
    #[inline(always)]
    pub fn len(&self) -> u32 {
        if self.pointer.inner.is_null() {
            return 0;
        }

        self.len
    }

    /// Checks if the array is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0 || self.pointer.inner.is_null()
    }

    /// Returns a mutable pointer to the length.
    ///
    /// This is useful when calling COM functions that output the length via a pointer.
    #[inline(always)]
    pub(crate) fn as_mut_len_ptr(&mut self) -> *mut u32 {
        &mut self.len
    }

    /// Sets the length of the array.
    ///
    /// # Safety
    /// The caller must ensure that the new length is valid for the underlying array.
    #[inline(always)]
    pub(crate) unsafe fn set_len(&mut self, len: u32) {
        self.len = len;
    }

    /// Converts the array into ordinary Rust-owned values.
    ///
    /// The conversion borrows each element while this wrapper remains the
    /// unique owner of the COM allocation. Dropping the wrapper releases every
    /// nested allocation and the outer buffer exactly once, including when a
    /// conversion fails partway through.
    pub fn into_vec<U>(
        self,
        convert: impl FnMut(&T) -> windows::core::Result<U>,
    ) -> windows::core::Result<Vec<U>> {
        self.as_slice().iter().map(convert).collect()
    }

    fn initialized_len(&self) -> usize {
        let logical_len = self
            .capacity
            .map_or(self.len, |capacity| self.len.min(capacity));
        let logical_len = usize::try_from(logical_len).unwrap_or(0);

        if self.pointer.inner.is_null() || logical_len == 0 {
            return 0;
        }

        let Some(required_bytes) = logical_len.checked_mul(core::mem::size_of::<T>()) else {
            return 0;
        };
        if required_bytes > isize::MAX as usize {
            return 0;
        }

        // `GetSize` is only a physical safety bound. It does not tell us how
        // many logical elements COM initialized, so the API-reported count
        // and caller-provided capacity remain authoritative.
        match unsafe { task_mem_allocation_bytes(self.pointer.inner.cast()) } {
            Some(allocated_bytes) if required_bytes <= allocated_bytes => logical_len,
            _ => 0,
        }
    }

    /// Returns the element count written by COM before capacity clamping.
    #[inline(always)]
    pub(crate) fn reported_len(&self) -> u32 {
        self.len
    }

    /// Returns the maximum number of elements known to fit in the allocation.
    #[inline(always)]
    pub(crate) fn capacity(&self) -> Option<u32> {
        self.capacity
    }

    /// Validates the pointer/count pair returned by COM.
    pub(crate) fn validate_output(&self, operation: &str) -> windows::core::Result<()> {
        if self.len > 0 && self.pointer.inner.is_null() {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_POINTER,
                format!(
                    "{operation} returned a null array for {} elements",
                    self.len
                ),
            ));
        }
        if let Some(capacity) = self.capacity
            && self.len > capacity
        {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!(
                    "{operation} returned {} elements for a {capacity}-element allocation",
                    self.len
                ),
            ));
        }
        if !self.pointer.inner.is_null() {
            let element_size = core::mem::size_of::<T>();
            let required_bytes = usize::try_from(self.len)
                .ok()
                .and_then(|len| len.checked_mul(element_size))
                .and_then(|bytes| (bytes <= isize::MAX as usize).then_some(bytes))
                .ok_or_else(|| {
                    windows::core::Error::new(
                        windows::Win32::Foundation::E_INVALIDARG,
                        format!(
                            "{operation} returned an element count that overflows its byte length"
                        ),
                    )
                })?;
            let allocated_bytes =
                // SAFETY: RemoteArray only owns COM task allocations.
                unsafe { task_mem_allocation_bytes(self.pointer.inner.cast()) }.ok_or_else(|| {
                    windows::core::Error::new(
                        windows::Win32::Foundation::E_INVALIDARG,
                        format!("{operation} returned a pointer outside the COM task allocator"),
                    )
                })?;
            if required_bytes > allocated_bytes {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    format!(
                        "{operation} returned {} elements requiring {required_bytes} bytes, \
                         but the COM allocation is only {allocated_bytes} bytes",
                        self.len
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl<T: Sized> core::fmt::Debug for RemoteArray<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RemoteArray")
            .field("pointer", &self.pointer)
            .field("len", &self.len)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl<T: Sized> PartialEq for RemoteArray<T> {
    fn eq(&self, other: &Self) -> bool {
        self.pointer == other.pointer && self.len == other.len
    }
}

impl<T: Sized> Drop for RemoteArray<T> {
    fn drop(&mut self) {
        let Some(cleanup) = self.element_cleanup else {
            return;
        };
        for element in self.as_mut_slice() {
            // SAFETY: The cleanup function matches T and each initialized
            // element is visited exactly once before the outer allocation.
            unsafe { cleanup(element) };
        }
    }
}

impl<T: Sized> Default for RemoteArray<T> {
    /// Creates an empty `RemoteArray` by default.
    #[inline(always)]
    fn default() -> Self {
        Self::empty()
    }
}

/// A safe wrapper around a pointer allocated by COM.
///
/// This struct ensures proper cleanup of COM-allocated memory when dropped.
/// It provides methods to access the underlying pointer.
pub struct RemotePointer<T: Sized> {
    inner: *mut T,
    cleanup: Option<PointerCleanup<T>>,
}

impl<T: Sized> RemotePointer<T> {
    /// Creates a new `RemotePointer` initialized to null.
    #[inline(always)]
    pub fn null() -> Self {
        Self {
            inner: core::ptr::null_mut(),
            cleanup: None,
        }
    }

    /// Returns a mutable pointer to the inner pointer.
    ///
    /// Useful for COM functions that output data via a pointer to a pointer.
    #[inline(always)]
    pub(crate) unsafe fn from_raw(pointer: *mut T) -> Self {
        Self {
            inner: pointer,
            cleanup: None,
        }
    }

    /// Creates an owning pointer with cleanup for fields nested in `T`.
    #[inline(always)]
    pub(crate) unsafe fn from_raw_with_cleanup(
        pointer: *mut T,
        cleanup: PointerCleanup<T>,
    ) -> Self {
        Self {
            inner: pointer,
            cleanup: Some(cleanup),
        }
    }

    pub(crate) fn copy_slice(value: &[T]) -> Self {
        if value.is_empty() {
            return Self::null();
        }

        // SAFETY: Allocates memory for slice using COM CoTaskMemAlloc.
        let pointer = unsafe { CoTaskMemAlloc(core::mem::size_of_val(value)) };
        if pointer.is_null() {
            return Self::null();
        }
        // SAFETY: Destination buffer was allocated with sufficient capacity and pointers are non-overlapping.
        unsafe {
            core::ptr::copy_nonoverlapping(value.as_ptr(), pointer as _, value.len());
        }
        Self {
            inner: pointer as _,
            cleanup: None,
        }
    }

    #[inline(always)]
    pub(crate) fn as_mut_ptr(&mut self) -> *mut *mut T {
        &mut self.inner
    }

    #[inline(always)]
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.inner
    }

    /// Releases a previously owned allocation after an in/out COM parameter
    /// replaces it with a different pointer.
    ///
    /// # Safety
    /// `previous` must have been owned by this wrapper immediately before the
    /// COM call that may have replaced the pointer.
    pub(crate) unsafe fn free_replaced(&self, previous: *mut T) {
        if !previous.is_null() && previous != self.inner {
            if let Some(cleanup) = self.cleanup {
                // SAFETY: `previous` had the same nested cleanup contract as
                // the output pointer that replaced it.
                unsafe { cleanup(previous) };
            }
            // SAFETY: The previous allocation is no longer reachable through
            // this owner and was allocated with the COM task allocator.
            unsafe { CoTaskMemFree(Some(previous.cast())) };
        }
    }

    /// Returns an `Option` referencing the inner value if it is not null.
    ///
    /// # Safety
    /// The caller must ensure that the inner pointer is valid for reads.
    #[inline(always)]
    pub fn as_ref(&self) -> Option<&T> {
        // SAFETY: Converting raw pointer to reference after validating pointer safety.
        unsafe { self.inner.as_ref() }
    }

    #[inline(always)]
    pub fn ok(&self) -> windows::core::Result<&T> {
        // SAFETY: Converting raw pointer to reference after validating pointer safety.
        unsafe { self.inner.as_ref() }.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_POINTER, "Pointer is null")
        })
    }

    #[inline(always)]
    pub fn from_option<R: Into<RemotePointer<T>>>(value: Option<R>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::null(),
        }
    }
}

impl<T: Sized> core::fmt::Debug for RemotePointer<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("RemotePointer")
            .field(&self.inner)
            .finish()
    }
}

impl<T: Sized> PartialEq for RemotePointer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: Sized> Default for RemotePointer<T> {
    /// Creates a new `RemotePointer` initialized to null by default.
    #[inline(always)]
    fn default() -> Self {
        Self::null()
    }
}

impl From<&str> for RemotePointer<u16> {
    /// Converts a string slice to a `RemotePointer<u16>`.
    #[inline(always)]
    fn from(value: &str) -> Self {
        Self::copy_slice(&value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>())
    }
}

impl TryFrom<RemotePointer<u16>> for String {
    type Error = windows::core::Error;

    /// Attempts to convert a `RemotePointer<u16>` to a `String`.
    ///
    /// # Errors
    /// Returns an error if the pointer is null or if the string conversion fails.
    #[inline(always)]
    fn try_from(value: RemotePointer<u16>) -> Result<Self, Self::Error> {
        if value.inner.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }

        // SAFETY: Has checked for non-null pointer above.
        Ok(unsafe { PWSTR(value.inner).to_string() }?)
    }
}

impl TryFrom<RemotePointer<u16>> for Option<String> {
    type Error = windows::core::Error;

    /// Attempts to convert a `RemotePointer<u16>` to an `Option<String>`.
    ///
    /// # Errors
    /// Returns an error if the string conversion fails.
    #[inline(always)]
    fn try_from(value: RemotePointer<u16>) -> Result<Self, Self::Error> {
        if value.inner.is_null() {
            return Ok(None);
        }

        // SAFETY: Has checked for non-null pointer above.
        Ok(Some(unsafe { PWSTR(value.inner).to_string() }?))
    }
}

impl RemotePointer<u16> {
    /// Returns a mutable pointer to a `PWSTR`.
    #[inline(always)]
    pub(crate) fn as_mut_pwstr_ptr(&mut self) -> *mut PWSTR {
        &mut self.inner as *mut *mut u16 as *mut PWSTR
    }
}

impl<T: Sized> Drop for RemotePointer<T> {
    /// Drops the `RemotePointer`, freeing the COM-allocated memory.
    #[inline(always)]
    fn drop(&mut self) {
        if !self.inner.is_null() {
            if let Some(cleanup) = self.cleanup {
                // SAFETY: The cleanup function matches T and runs exactly once
                // before the containing COM allocation is released.
                unsafe { cleanup(self.inner) };
            }
            // SAFETY: Memory was allocated via COM CoTaskMemAlloc and pointer is non-null.
            unsafe {
                CoTaskMemFree(Some(self.inner as _));
            }
        }
    }
}

/// Frees a nested COM task-allocated wide string and nulls its slot.
pub(crate) unsafe fn clear_pwstr(value: &mut PWSTR) {
    if !value.is_null() {
        // SAFETY: OPC DA returns these strings from the COM task allocator.
        unsafe { CoTaskMemFree(Some(value.as_ptr().cast())) };
        #[cfg(test)]
        PWSTR_CLEANUP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *value = PWSTR::null();
    }
}

#[cfg(test)]
pub(crate) static PWSTR_CLEANUP_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Clears a nested COM VARIANT without treating its enclosing raw buffer as a
/// Rust-owned `VARIANT` value.
pub(crate) unsafe fn clear_variant(value: &mut VARIANT) {
    // SAFETY: The value was initialized by COM and is cleared exactly once.
    unsafe {
        let _ = VariantClear(value);
    }
}

/// Returns the physical byte size of a COM task allocation.
///
/// This is a safety bound for validating a caller-supplied byte or element
/// count. It is not an initialized-element count: COM allocators may round
/// allocations up, and only the API contract can define which elements exist.
pub(crate) unsafe fn task_mem_allocation_bytes(pointer: *const core::ffi::c_void) -> Option<usize> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: CoGetMalloc returns the task allocator used by CoTaskMemAlloc,
    // and GetSize only observes the supplied allocation.
    let bytes = unsafe { CoGetMalloc(1).ok()?.GetSize(Some(pointer)) };
    (bytes != usize::MAX).then_some(bytes)
}

/// Creates an independently owned copy of a COM `VARIANT`.
///
/// Unlike `VARIANT::clone`, this preserves a `VariantCopy` failure instead of
/// silently returning an empty value.
pub(crate) fn clone_variant(value: &VARIANT) -> windows::core::Result<VARIANT> {
    let mut copy = VARIANT::default();
    // SAFETY: Both pointers reference initialized VARIANT values.
    let result = unsafe { VariantCopy(&mut copy, value) };
    if let Err(error) = result {
        // VariantCopy may have partially initialized its destination before
        // reporting an error, so release any nested allocation it produced.
        // SAFETY: `copy` started as VT_EMPTY and is the sole owner of anything
        // VariantCopy may have placed in it.
        unsafe {
            let _ = VariantClear(&mut copy);
        }
        return Err(error);
    }
    Ok(copy)
}

/// A safe wrapper around locally allocated memory needing to be passed to COM functions.
///
/// This struct is useful for preparing data to be read by COM functions.
pub struct LocalPointer<T: Sized> {
    inner: Option<Box<T>>,
}

impl<T: Sized> LocalPointer<T> {
    /// Creates a new `LocalPointer` from an optional value.
    #[inline(always)]
    pub fn new(value: Option<T>) -> Self {
        Self {
            inner: value.map(Box::new),
        }
    }

    /// Creates a `LocalPointer` from a boxed value.
    #[inline(always)]
    pub fn from_box(value: Box<T>) -> Self {
        Self { inner: Some(value) }
    }

    #[inline(always)]
    pub fn from_option<R: Into<LocalPointer<T>>>(value: Option<R>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::new(None),
        }
    }

    /// Returns a constant pointer to the inner value.
    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        match &self.inner {
            Some(value) => value.as_ref() as *const T,
            None => std::ptr::null_mut(),
        }
    }

    /// Returns a mutable pointer to the inner value.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        match &mut self.inner {
            Some(value) => value.as_mut() as *mut T,
            None => std::ptr::null_mut(),
        }
    }

    /// Consumes the `LocalPointer`, returning the inner value if it exists.
    #[inline(always)]
    pub fn into_inner(self) -> Option<T> {
        self.inner.map(|v| *v)
    }

    /// Returns a reference to the inner value if it exists.
    #[inline(always)]
    pub fn inner(&self) -> Option<&T> {
        self.inner.as_ref().map(|v| v.as_ref())
    }
}

// Implementations for string handling

impl<S: AsRef<str>> From<S> for LocalPointer<Vec<u16>> {
    /// Converts a string slice to a `LocalPointer` containing a UTF-16 encoded null-terminated string.
    #[inline(always)]
    fn from(s: S) -> Self {
        Self::new(Some(s.as_ref().encode_utf16().chain(Some(0)).collect()))
    }
}

impl From<&[String]> for LocalPointer<Vec<Vec<u16>>> {
    /// Converts a slice of `String`s to a `LocalPointer` containing vectors of UTF-16 encoded null-terminated strings.
    #[inline(always)]
    fn from(values: &[String]) -> Self {
        Self::new(Some(
            values
                .iter()
                .map(|s| s.encode_utf16().chain(Some(0)).collect())
                .collect(),
        ))
    }
}

impl<T> LocalPointer<Vec<T>> {
    /// Returns the length of the inner vector.
    #[inline(always)]
    pub fn len(&self) -> usize {
        match &self.inner {
            Some(values) => values.len(),
            None => 0,
        }
    }

    /// Checks if the inner vector is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        match &self.inner {
            Some(values) => values.is_empty(),
            None => true,
        }
    }

    /// Returns a constant pointer to the inner array.
    #[inline(always)]
    pub fn as_array_ptr(&self) -> *const T {
        match &self.inner {
            Some(values) => values.as_ptr(),
            None => std::ptr::null(),
        }
    }

    /// Returns a mutable pointer to the inner array.
    #[inline(always)]
    pub fn as_mut_array_ptr(&mut self) -> *mut T {
        match &mut self.inner {
            Some(values) => values.as_mut_ptr(),
            None => std::ptr::null_mut(),
        }
    }
}

impl LocalPointer<Vec<Vec<u16>>> {
    /// Converts the inner vector of UTF-16 strings to a vector of `PWSTR`.
    #[inline(always)]
    pub fn as_pwstr_array(&self) -> Vec<windows::core::PWSTR> {
        match &self.inner {
            Some(values) => values
                .iter()
                .map(|value| windows::core::PWSTR(value.as_ptr() as _))
                .collect(),
            None => vec![windows::core::PWSTR::null()],
        }
    }

    /// Converts the inner vector of UTF-16 strings to a vector of `PCWSTR`.
    #[inline(always)]
    pub fn as_pcwstr_array(&self) -> Vec<windows::core::PCWSTR> {
        match &self.inner {
            Some(values) => values
                .iter()
                .map(|value| windows::core::PCWSTR::from_raw(value.as_ptr() as _))
                .collect(),
            None => vec![windows::core::PCWSTR::null()],
        }
    }
}

impl LocalPointer<Vec<u16>> {
    /// Converts the inner UTF-16 string to a `PWSTR`.
    #[inline(always)]
    pub fn as_pwstr(&self) -> windows::core::PWSTR {
        match &self.inner {
            Some(value) => windows::core::PWSTR(value.as_ptr() as _),
            None => windows::core::PWSTR::null(),
        }
    }

    /// Converts the inner UTF-16 string to a `PCWSTR`.
    #[inline(always)]
    pub fn as_pcwstr(&self) -> windows::core::PCWSTR {
        match &self.inner {
            Some(value) => windows::core::PCWSTR::from_raw(value.as_ptr() as _),
            None => windows::core::PCWSTR::null(),
        }
    }
}

// ── Native Conversion Traits ────────────────────────────────────────

pub(crate) trait IntoBridge<Bridge> {
    fn into_bridge(self) -> Bridge;
}

pub(crate) trait ToNative<Native> {
    fn to_native(&self) -> Native;
}

pub(crate) trait FromNative<Native> {
    fn from_native(native: &Native) -> Self
    where
        Self: Sized;
}

pub(crate) trait TryToNative<Native> {
    fn try_to_native(&self) -> windows::core::Result<Native>;
}

pub(crate) trait TryFromNative<Native> {
    fn try_from_native(native: &Native) -> windows::core::Result<Self>
    where
        Self: Sized;
}

pub(crate) trait TryToLocal<Local> {
    fn try_to_local(&self) -> windows::core::Result<Local>;
}

impl<Native, T: TryFromNative<Native>> TryToLocal<T> for Native {
    fn try_to_local(&self) -> windows::core::Result<T> {
        T::try_from_native(self)
    }
}

impl<Native, T: FromNative<Native>> TryFromNative<Native> for T {
    fn try_from_native(native: &Native) -> windows::core::Result<Self> {
        Ok(Self::from_native(native))
    }
}

impl<Native, T: ToNative<Native>> TryToNative<Native> for T {
    fn try_to_native(&self) -> windows::core::Result<Native> {
        Ok(self.to_native())
    }
}

impl<Bridge, B: IntoBridge<Bridge>> IntoBridge<Vec<Bridge>> for Vec<B> {
    fn into_bridge(self) -> Vec<Bridge> {
        self.into_iter().map(IntoBridge::into_bridge).collect()
    }
}

impl<Bridge, B: IntoBridge<Bridge> + Clone> IntoBridge<Vec<Bridge>> for &[B] {
    fn into_bridge(self) -> Vec<Bridge> {
        self.iter().cloned().map(IntoBridge::into_bridge).collect()
    }
}

impl<Native, T: TryToNative<Native>> TryToNative<Vec<Native>> for Vec<T> {
    fn try_to_native(&self) -> windows::core::Result<Vec<Native>> {
        self.iter().map(TryToNative::try_to_native).collect()
    }
}

impl TryFromNative<RemoteArray<windows::core::HRESULT>> for Vec<windows::core::Result<()>> {
    fn try_from_native(
        native: &RemoteArray<windows::core::HRESULT>,
    ) -> windows::core::Result<Self> {
        native.validate_output("HRESULT array conversion")?;
        Ok(native.as_slice().iter().map(|v| (*v).ok()).collect())
    }
}

impl<Native, T: TryFromNative<Native>> TryFromNative<RemoteArray<Native>> for Vec<T> {
    fn try_from_native(native: &RemoteArray<Native>) -> windows::core::Result<Self> {
        native.validate_output("native array conversion")?;
        native.as_slice().iter().map(T::try_from_native).collect()
    }
}

impl<Native, T: TryFromNative<Native>>
    TryFromNative<(RemoteArray<Native>, RemoteArray<windows::core::HRESULT>)>
    for Vec<windows::core::Result<T>>
{
    fn try_from_native(
        native: &(RemoteArray<Native>, RemoteArray<windows::core::HRESULT>),
    ) -> windows::core::Result<Self> {
        let (results, errors) = native;
        results.validate_output("result array conversion")?;
        errors.validate_output("error array conversion")?;
        if results.len() != errors.len() {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "Results and errors arrays have different lengths",
            ));
        }

        Ok(results
            .as_slice()
            .iter()
            .zip(errors.as_slice())
            .map(|(result, error)| {
                if error.is_ok() {
                    T::try_from_native(result)
                } else {
                    Err((*error).into())
                }
            })
            .collect())
    }
}

impl TryFromNative<windows::Win32::Foundation::FILETIME> for std::time::SystemTime {
    fn try_from_native(
        native: &windows::Win32::Foundation::FILETIME,
    ) -> windows::core::Result<Self> {
        let ft = ((native.dwHighDateTime as u64) << 32) | (u64::from(native.dwLowDateTime));
        let duration_since_1601 = std::time::Duration::from_nanos(ft * 100);

        let windows_to_unix_epoch_diff = std::time::Duration::from_secs(11_644_473_600);
        let duration_since_unix_epoch = duration_since_1601
            .checked_sub(windows_to_unix_epoch_diff)
            .ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "FILETIME is before UNIX_EPOCH",
                )
            })?;

        Ok(std::time::UNIX_EPOCH + duration_since_unix_epoch)
    }
}

#[macro_export]
/// Helper macro for instantiating native COM structs from safe types.
macro_rules! try_from_native {
    ($native:expr) => {
        $crate::opc_da::com_utils::TryFromNative::try_from_native($native)?
    };
}

impl TryToNative<windows::Win32::Foundation::FILETIME> for std::time::SystemTime {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::Foundation::FILETIME> {
        let duration_since_unix_epoch =
            self.duration_since(std::time::UNIX_EPOCH).map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "SystemTime is before UNIX_EPOCH",
                )
            })?;

        let duration_since_windows_epoch =
            duration_since_unix_epoch + std::time::Duration::from_secs(11_644_473_600);

        let ft = duration_since_windows_epoch.as_nanos() / 100;

        Ok(windows::Win32::Foundation::FILETIME {
            dwLowDateTime: ft as u32,
            dwHighDateTime: (ft >> 32) as u32,
        })
    }
}

impl TryFromNative<windows::core::PWSTR> for String {
    fn try_from_native(native: &windows::core::PWSTR) -> windows::core::Result<Self> {
        if native.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        // SAFETY: This conversion borrows the COM-owned string. Its containing
        // owner remains responsible for releasing the allocation.
        Ok(unsafe { native.to_string() }?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CLEANED_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
    static CAPPED_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
    static FAILED_CONVERSION_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
    static UNWRITTEN_ELEMENTS: AtomicUsize = AtomicUsize::new(0);

    unsafe fn count_cleanup(_: &mut u32) {
        CLEANED_ELEMENTS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe fn count_capped_cleanup(_: &mut u32) {
        CAPPED_ELEMENTS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe fn count_failed_conversion_cleanup(_: &mut u32) {
        FAILED_CONVERSION_ELEMENTS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe fn count_unwritten_cleanup(_: &mut u32) {
        UNWRITTEN_ELEMENTS.fetch_add(1, Ordering::SeqCst);
    }

    fn remote_u32_array(values: &[u32], cleanup: unsafe fn(&mut u32)) -> RemoteArray<u32> {
        // SAFETY: Allocate enough COM task memory for every copied element.
        let pointer = unsafe { CoTaskMemAlloc(core::mem::size_of_val(values)) }.cast::<u32>();
        assert!(!pointer.is_null());
        // SAFETY: The allocation is large enough and does not overlap values.
        unsafe {
            core::ptr::copy_nonoverlapping(values.as_ptr(), pointer, values.len());
        }
        // SAFETY: pointer is a COM task allocation containing exactly the
        // initialized values copied above, and ownership transfers here.
        unsafe {
            RemoteArray::from_mut_ptr_with_cleanup(
                pointer,
                u32::try_from(values.len()).unwrap(),
                cleanup,
            )
        }
    }

    #[test]
    fn into_vec_copies_values_without_nested_com_ownership() {
        let values = [10, 20, 30];
        // SAFETY: Allocate enough COM task memory for every copied element.
        let pointer = unsafe { CoTaskMemAlloc(core::mem::size_of_val(&values)) }.cast::<u32>();
        assert!(!pointer.is_null());
        // SAFETY: The allocation is large enough and does not overlap values.
        unsafe {
            core::ptr::copy_nonoverlapping(values.as_ptr(), pointer, values.len());
        }
        // SAFETY: pointer is a COM task allocation containing exactly the
        // initialized values copied above, and ownership transfers here.
        let remote =
            unsafe { RemoteArray::from_mut_ptr(pointer, u32::try_from(values.len()).unwrap()) };

        assert_eq!(remote.into_vec(|value| Ok(*value)).unwrap(), values);
    }

    #[test]
    fn into_vec_cleans_nested_com_ownership_after_conversion() {
        CLEANED_ELEMENTS.store(0, Ordering::SeqCst);
        let remote = remote_u32_array(&[10, 20, 30], count_cleanup);

        let local = remote.into_vec(|value| Ok(*value)).unwrap();

        assert_eq!(local, vec![10, 20, 30]);
        assert_eq!(CLEANED_ELEMENTS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn into_vec_cleans_every_element_when_conversion_fails() {
        FAILED_CONVERSION_ELEMENTS.store(0, Ordering::SeqCst);
        let remote = remote_u32_array(&[10, 20, 30], count_failed_conversion_cleanup);

        let error = remote
            .into_vec(|value| {
                if *value == 20 {
                    Err(windows::core::Error::new(
                        windows::Win32::Foundation::E_INVALIDARG,
                        "conversion failed",
                    ))
                } else {
                    Ok(*value)
                }
            })
            .expect_err("the middle element should fail conversion");

        assert_eq!(error.code(), windows::Win32::Foundation::E_INVALIDARG);
        assert_eq!(FAILED_CONVERSION_ELEMENTS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn cleanup_is_capped_at_the_known_allocation_capacity() {
        CAPPED_ELEMENTS.store(0, Ordering::SeqCst);
        let mut remote = remote_u32_array(&[1, 2], count_capped_cleanup);
        // SAFETY: Deliberately simulates a malformed COM fetched count. The
        // wrapper must never walk beyond its known two-element allocation.
        unsafe { remote.set_len(5) };

        assert_eq!(remote.reported_len(), 5);
        assert_eq!(remote.len(), 5);
        assert_eq!(remote.as_slice().len(), 2);
        assert_eq!(
            remote
                .validate_output("test")
                .expect_err("the malformed length must be rejected")
                .code(),
            windows::Win32::Foundation::E_INVALIDARG
        );
        drop(remote);

        assert_eq!(CAPPED_ELEMENTS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn output_capacity_does_not_count_unwritten_elements_as_initialized() {
        UNWRITTEN_ELEMENTS.store(0, Ordering::SeqCst);
        let mut remote = RemoteArray::with_capacity_and_cleanup(2, count_unwritten_cleanup);
        let values = [1_u32, 2];
        // SAFETY: Allocate and initialize the simulated COM output buffer.
        let pointer = unsafe { CoTaskMemAlloc(core::mem::size_of_val(&values)) }.cast::<u32>();
        assert!(!pointer.is_null());
        // SAFETY: The allocation is large enough and the destination is the
        // wrapper's currently-null output pointer.
        unsafe {
            core::ptr::copy_nonoverlapping(values.as_ptr(), pointer, values.len());
            *remote.as_mut_ptr() = pointer;
        }

        drop(remote);

        assert_eq!(UNWRITTEN_ELEMENTS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn borrowed_pwstr_conversion_leaves_freeing_to_the_owner() {
        let owner = RemotePointer::<u16>::from("borrowed");
        let borrowed = PWSTR(owner.as_ptr());

        assert_eq!(
            String::try_from_native(&borrowed).unwrap(),
            "borrowed".to_string()
        );
        assert_eq!(unsafe { borrowed.to_string() }.unwrap(), "borrowed");
        drop(owner);
    }

    #[test]
    fn clone_variant_rejects_an_invalid_variant() {
        let mut invalid = VARIANT::default();
        // SAFETY: Deliberately construct an invalid VARTYPE with no owned
        // payload to exercise the VariantCopy failure path.
        unsafe {
            (*invalid.Anonymous.Anonymous).vt = windows::Win32::System::Variant::VARENUM(u16::MAX);
        }

        let error = clone_variant(&invalid).expect_err("invalid VARTYPE must be rejected");

        assert!(error.code().is_err());
    }
}
