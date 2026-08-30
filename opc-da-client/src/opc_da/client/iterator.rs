use crate::opc_da::{
    com_utils::{RemoteArray, RemotePointer, TryToLocal as _},
    errors::{
        MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES, OpcError, OpcResult, browse_non_progress_error,
    },
};
use std::time::Instant;
use windows::core::Interface as _;

const MAX_CACHE_SIZE: usize = 16;
const STRING_CACHE_SIZE: usize = 256;

fn validate_fetched_count(count: u32, capacity: usize, iterator: &str) -> OpcResult<()> {
    if count > capacity as u32 {
        return Err(OpcError::Internal(format!(
            "{iterator} returned {count} entries for a {capacity}-entry cache"
        )));
    }
    Ok(())
}

/// Iterator over COM GUIDs from IEnumGUID.  
///
/// # Safety  
/// This struct wraps a COM interface and must be used according to COM rules.  
pub struct GuidIterator {
    inner: windows::Win32::System::Com::IEnumGUID,
    cache: Box<[windows::core::GUID; MAX_CACHE_SIZE]>,
    index: u32,
    count: u32,
    done: bool,
}

impl GuidIterator {
    /// Creates a new iterator from a COM interface.  
    pub fn new(inner: windows::Win32::System::Com::IEnumGUID) -> Self {
        Self {
            inner,
            cache: Box::from([windows::core::GUID::zeroed(); MAX_CACHE_SIZE]),
            index: MAX_CACHE_SIZE as u32,
            count: 0,
            done: false,
        }
    }
}

impl Iterator for GuidIterator {
    type Item = OpcResult<windows::core::GUID>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        if self.index >= self.count {
            // SAFETY: Calling IEnumGUID::Next COM interface method with valid mutable cache slice and count pointer.
            let code = unsafe {
                self.inner
                    .Next(self.cache.as_mut_slice(), Some(&mut self.count))
            };

            if code.is_ok() {
                if let Err(error) =
                    validate_fetched_count(self.count, self.cache.len(), "IEnumGUID")
                {
                    self.done = true;
                    return Some(Err(error));
                }
                if self.count == 0 {
                    self.done = true;
                    return None;
                }

                self.index = 0;
            } else {
                self.done = true;
                return Some(Err(windows::core::Error::new(
                    code,
                    "Failed to get next GUID",
                )
                .into()));
            }
        }

        let current = self.cache[self.index as usize];
        self.index += 1;
        Some(Ok(current))
    }
}

pub struct StringIterator {
    inner: windows::Win32::System::Com::IEnumString,
    cache: Box<[windows::core::PWSTR; STRING_CACHE_SIZE]>,
    index: u32,
    count: u32,
    done: bool,
    populated: usize,
    last_value: Option<String>,
    consecutive: usize,
    yielded: usize,
    empty_batches: usize,
}

impl StringIterator {
    pub fn new(inner: windows::Win32::System::Com::IEnumString) -> Self {
        Self {
            inner,
            cache: Box::new([windows::core::PWSTR::null(); STRING_CACHE_SIZE]),
            index: STRING_CACHE_SIZE as u32,
            count: 0,
            done: false,
            populated: 0,
            last_value: None,
            consecutive: 0,
            yielded: 0,
            empty_batches: 0,
        }
    }
}

impl Drop for StringIterator {
    fn drop(&mut self) {
        self.release_pending();
    }
}

impl StringIterator {
    fn release_pending(&mut self) {
        let start = (self.index as usize).min(self.populated);
        for slot in self.cache[start..self.populated].iter_mut() {
            let pwstr = std::mem::replace(slot, windows::core::PWSTR::null());
            if !pwstr.is_null() {
                drop(RemotePointer::from(pwstr));
            }
        }
        self.populated = 0;
    }

    pub(crate) fn next_with_gate(
        &mut self,
        before_refill: &mut dyn FnMut(u32) -> bool,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Option<<Self as Iterator>::Item> {
        loop {
            if self.done {
                return None;
            }
            if should_cancel() {
                return None;
            }

            if self.index >= self.count {
                if !before_refill(self.cache.len() as u32) {
                    return None;
                }

                // Zero the cache to prevent stale freed pointers (OPC-BUG-001)
                let started = Instant::now();
                tracing::debug!(celt = self.cache.len(), "Starting IEnumString::Next");
                self.cache.fill(windows::core::PWSTR::null());
                self.index = 0;
                self.count = 0;
                self.populated = 0;

                // SAFETY: Calling IEnumString::Next COM interface method with valid mutable cache slice and count pointer.
                let code = unsafe {
                    self.inner
                        .Next(self.cache.as_mut_slice(), Some(&mut self.count))
                };

                tracing::debug!(
                    hresult = format_args!("{:#010X}", code.0),
                    celt = self.cache.len(),
                    fetched = self.count,
                    elapsed_ms = started.elapsed().as_millis(),
                    "IEnumString::Next returned"
                );

                self.populated = (self.count as usize).min(self.cache.len());
                if code.is_ok() {
                    if let Err(error) =
                        validate_fetched_count(self.count, self.cache.len(), "IEnumString")
                    {
                        self.done = true;
                        return Some(Err(error));
                    }
                    if self.count == 0 {
                        self.done = true;
                        return None;
                    }

                    // Detect null entries in the fetched range
                    let null_count = self.cache[..self.populated]
                        .iter()
                        .filter(|p| p.is_null())
                        .count();
                    if null_count > 0 {
                        tracing::warn!(
                            null_count,
                            fetched = self.count,
                            "StringIterator: null PWSTR entries in fetched range"
                        );
                    }
                    if null_count == self.populated {
                        self.empty_batches += 1;
                        if self.empty_batches >= MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES {
                            self.done = true;
                            return Some(Err(browse_non_progress_error(
                                "IEnumString",
                                &[],
                                "<null PWSTR>",
                                self.empty_batches,
                                self.yielded,
                            )));
                        }
                    } else {
                        self.empty_batches = 0;
                    }

                    self.index = 0;
                } else {
                    self.done = true;
                    return Some(Err(windows::core::Error::new(
                        code,
                        "Failed to get next string",
                    )
                    .into()));
                }
            }

            // Skip null PWSTR entries instead of producing E_POINTER (OPC-BUG-001)
            let pwstr = std::mem::replace(
                &mut self.cache[self.index as usize],
                windows::core::PWSTR::null(),
            );
            self.index += 1;

            if pwstr.is_null() {
                tracing::debug!(
                    index = self.index - 1,
                    count = self.count,
                    "StringIterator: skipping null PWSTR entry"
                );
                continue; // Loop back to try the next entry
            }

            let current = RemotePointer::from(pwstr);
            let value: String = match current.try_into() {
                Ok(value) => value,
                Err(error) => return Some(Err(OpcError::from(error))),
            };

            self.yielded += 1;
            if self.last_value.as_deref() == Some(value.as_str()) {
                self.consecutive += 1;
            } else {
                self.last_value = Some(value.clone());
                self.consecutive = 1;
            }
            if self.consecutive >= MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES {
                self.done = true;
                return Some(Err(browse_non_progress_error(
                    "IEnumString",
                    &[],
                    &value,
                    self.consecutive,
                    self.yielded,
                )));
            }

            return Some(Ok(value));
        }
    }
}

impl Iterator for StringIterator {
    type Item = OpcResult<String>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_with_gate(&mut |_| true, &mut || false)
    }
}

pub struct GroupIterator<Group: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> {
    inner: windows::Win32::System::Com::IEnumUnknown,
    cache: Box<[Option<windows::core::IUnknown>; MAX_CACHE_SIZE]>,
    index: u32,
    count: u32,
    done: bool,
    _mark: std::marker::PhantomData<Group>,
}

impl<Group: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> GroupIterator<Group> {
    pub fn new(inner: windows::Win32::System::Com::IEnumUnknown) -> Self {
        Self {
            inner,
            cache: Box::from([const { None }; MAX_CACHE_SIZE]),
            index: MAX_CACHE_SIZE as u32,
            count: 0,
            done: false,
            _mark: std::marker::PhantomData,
        }
    }
}

impl<Group: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> Iterator
    for GroupIterator<Group>
{
    type Item = OpcResult<Group>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        if self.index >= self.count {
            // SAFETY: Calling IEnumUnknown::Next COM interface method with valid mutable cache slice and count pointer.
            let code = unsafe {
                self.inner
                    .Next(self.cache.as_mut_slice(), Some(&mut self.count))
            };

            if code.is_ok() {
                if let Err(error) =
                    validate_fetched_count(self.count, self.cache.len(), "IEnumUnknown")
                {
                    self.done = true;
                    return Some(Err(error));
                }
                if self.count == 0 {
                    self.done = true;
                    return None;
                }

                self.index = 0;
            } else {
                self.done = true;
                return Some(Err(windows::core::Error::new(
                    code,
                    "Failed to get next group",
                )
                .into()));
            }
        }

        let current = self.cache[self.index as usize].take();
        self.index += 1;
        Some(match current {
            Some(group) => group.try_into().map_err(OpcError::from),
            None => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_POINTER,
                "Failed to get group, returned null",
            )
            .into()),
        })
    }
}

// for crate::bindings::da::IEnumOPCItemAttributes
pub struct ItemAttributeIterator {
    inner: crate::bindings::da::IEnumOPCItemAttributes,
    cache: RemoteArray<crate::bindings::da::tagOPCITEMATTRIBUTES>,
    index: u32,
    done: bool,
}

impl ItemAttributeIterator {
    pub fn new(inner: crate::bindings::da::IEnumOPCItemAttributes) -> Self {
        Self {
            inner,
            cache: RemoteArray::empty(),
            index: 0,
            done: false,
        }
    }
}

impl Iterator for ItemAttributeIterator {
    type Item = OpcResult<crate::opc_da::typedefs::ItemAttributes>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        if self.index >= self.cache.len() {
            let mut attrs = RemoteArray::new(MAX_CACHE_SIZE as u32);

            // SAFETY: Calling IEnumOPCItemAttributes::Next COM interface method with valid output array pointers.
            let result = unsafe {
                self.inner.Next(
                    MAX_CACHE_SIZE as u32,
                    attrs.as_mut_ptr(),
                    attrs.as_mut_len_ptr(),
                )
            };

            match result {
                Ok(_) => {
                    if attrs.is_empty() {
                        self.done = true;
                        return None;
                    }

                    self.cache = attrs;
                    self.index = 0;
                }
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            }
        }

        let current: windows::core::Result<crate::opc_da::typedefs::ItemAttributes> =
            self.cache.as_slice()[self.index as usize].try_to_local();
        self.index += 1;
        Some(current.map_err(OpcError::from))
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::ref_as_ptr,
        clippy::inline_always,
        clippy::useless_conversion,
        clippy::needless_range_loop
    )]
    use super::*;
    use windows::Win32::System::Com::{IEnumString, IEnumString_Impl};
    use windows::core::{PWSTR, implement};

    #[allow(clippy::ref_as_ptr, clippy::inline_always)]
    #[implement(IEnumString)]
    struct MockEnumString {
        items: Vec<String>,
        index: std::sync::atomic::AtomicUsize,
    }

    impl IEnumString_Impl for MockEnumString_Impl {
        fn Next(
            &self,
            celt: u32,
            rgelt: *mut PWSTR,
            pceltfetched: *mut u32,
        ) -> windows::core::HRESULT {
            let mut fetched = 0;
            let index = self.index.load(std::sync::atomic::Ordering::Relaxed);
            let rgelt = unsafe { std::slice::from_raw_parts_mut(rgelt, celt as usize) };

            for i in 0..celt as usize {
                if index + i < self.items.len() {
                    let s = &self.items[index + i];
                    let mut w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                    let ptr = unsafe { windows::Win32::System::Com::CoTaskMemAlloc(w.len() * 2) };
                    unsafe { std::ptr::copy_nonoverlapping(w.as_ptr(), ptr as *mut u16, w.len()) };
                    rgelt[i] = PWSTR(ptr as *mut u16);
                    fetched += 1;
                } else {
                    break;
                }
            }

            self.index
                .store(index + fetched, std::sync::atomic::Ordering::Relaxed);

            if !pceltfetched.is_null() {
                unsafe { *pceltfetched = fetched as u32 };
            }

            if fetched == celt as usize {
                windows::Win32::Foundation::S_OK.into()
            } else {
                windows::Win32::Foundation::S_FALSE.into()
            }
        }
        fn Skip(&self, _celt: u32) -> windows::core::HRESULT {
            windows::Win32::Foundation::E_NOTIMPL.into()
        }
        fn Reset(&self) -> windows::core::Result<()> {
            self.index.store(0, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        fn Clone(&self) -> windows::core::Result<IEnumString> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
    }

    #[test]
    fn test_string_iterator_no_phantom_errors() {
        let items = vec![
            "Item1".to_string(),
            "Item2".to_string(),
            "Item3".to_string(),
        ];

        let mock_enum: IEnumString = MockEnumString {
            items: items.clone(),
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let iter = StringIterator::new(mock_enum);

        let mut results = Vec::new();
        for item in iter {
            // Verify no E_POINTER error is yielded
            let value = item.expect("Expected OK value, got phantom error");
            results.push(value);
        }

        assert_eq!(results, items);
    }

    #[test]
    fn test_string_iterator_gate_runs_once_per_native_refill() {
        let items = vec![
            "Item1".to_string(),
            "Item2".to_string(),
            "Item3".to_string(),
        ];
        let mock_enum: IEnumString = MockEnumString {
            items: items.clone(),
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let mut iter = StringIterator::new(mock_enum);
        let mut costs = Vec::new();
        let mut gate = |cost| {
            costs.push(cost);
            true
        };
        let mut should_cancel = || false;
        let mut results = Vec::new();
        while let Some(item) = iter.next_with_gate(&mut gate, &mut should_cancel) {
            results.push(item.expect("the native item must be valid"));
        }

        assert_eq!(results, items);
        assert_eq!(
            costs,
            vec![STRING_CACHE_SIZE as u32, STRING_CACHE_SIZE as u32],
            "cached items must not each consume a native-operation budget"
        );
    }

    #[test]
    fn test_string_iterator_gate_can_cancel_before_refill() {
        let mock_enum: IEnumString = MockEnumString {
            items: vec!["Item1".to_string()],
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let mut iter = StringIterator::new(mock_enum);
        let mut called = false;
        let mut should_cancel = || false;
        {
            let mut gate = |_cost| {
                called = true;
                false
            };
            assert!(iter.next_with_gate(&mut gate, &mut should_cancel).is_none());
        }
        assert!(called, "the gate must run before the native refill");
        assert!(
            iter.next().is_some(),
            "cancellation must not permanently exhaust the iterator"
        );
    }

    #[test]
    fn test_string_iterator_checks_cancellation_for_cached_items() {
        let items = vec![
            "Item1".to_string(),
            "Item2".to_string(),
            "Item3".to_string(),
        ];
        let mock_enum: IEnumString = MockEnumString {
            items,
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let mut iter = StringIterator::new(mock_enum);
        let mut refill_count = 0;
        let mut gate = |_cost| {
            refill_count += 1;
            true
        };
        let mut cached_items_seen = 0;
        let mut should_cancel = || {
            cached_items_seen += 1;
            cached_items_seen > 1
        };

        assert_eq!(
            iter.next_with_gate(&mut gate, &mut should_cancel)
                .expect("the first item must be yielded")
                .expect("the first item must be valid"),
            "Item1"
        );
        assert!(
            iter.next_with_gate(&mut gate, &mut should_cancel).is_none(),
            "cancellation must stop before the next cached item"
        );
        assert_eq!(
            refill_count, 1,
            "cached-item cancellation must not trigger another native refill"
        );
    }

    #[test]
    fn test_string_iterator_terminates_on_non_progress() {
        let mock_enum: IEnumString = MockEnumString {
            items: std::iter::repeat_n(
                "\u{1}".to_string(),
                MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES,
            )
            .collect(),
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let mut iter = StringIterator::new(mock_enum);
        for _ in 0..(MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES - 1) {
            assert_eq!(
                iter.next()
                    .expect("the iterator must yield a value")
                    .expect("the value must be valid"),
                "\u{1}"
            );
        }

        let error = iter
            .next()
            .expect("the iterator must report non-progress")
            .expect_err("repeated values must terminate the iterator");
        assert!(matches!(
            error,
            OpcError::BrowseNonProgress {
                iterator_type,
                browse_path,
                repeated_value,
                consecutive,
                yielded,
            } if iterator_type == "IEnumString"
                && browse_path == "<root>"
                && repeated_value == "\u{1}"
                && consecutive == MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES
                && yielded == MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES
        ));
        assert!(iter.next().is_none());
    }

    /// Mock that writes only `valid_count` items but claims `pceltFetched = claimed_count`,
    /// leaving the remaining slots as null pointers. Simulates OPC-BUG-001.
    #[allow(clippy::ref_as_ptr, clippy::inline_always)]
    #[implement(IEnumString)]
    struct MockEnumStringWithNulls {
        items: Vec<String>,
        index: std::sync::atomic::AtomicUsize,
        /// How many *extra* null entries to claim beyond actual items
        extra_nulls: u32,
    }

    impl IEnumString_Impl for MockEnumStringWithNulls_Impl {
        fn Next(
            &self,
            celt: u32,
            rgelt: *mut PWSTR,
            pceltfetched: *mut u32,
        ) -> windows::core::HRESULT {
            let mut fetched = 0;
            let index = self.index.load(std::sync::atomic::Ordering::Relaxed);
            let rgelt = unsafe { std::slice::from_raw_parts_mut(rgelt, celt as usize) };

            for i in 0..celt as usize {
                if index + i < self.items.len() {
                    let s = &self.items[index + i];
                    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                    let ptr = unsafe { windows::Win32::System::Com::CoTaskMemAlloc(w.len() * 2) };
                    unsafe { std::ptr::copy_nonoverlapping(w.as_ptr(), ptr as *mut u16, w.len()) };
                    rgelt[i] = PWSTR(ptr as *mut u16);
                    fetched += 1;
                } else {
                    break;
                }
            }

            self.index
                .store(index + fetched, std::sync::atomic::Ordering::Relaxed);

            // Lie about the count: claim extra null entries (only on non-empty batches)
            let reported = if fetched > 0 {
                (fetched as u32) + self.extra_nulls
            } else {
                0
            };
            if !pceltfetched.is_null() {
                unsafe { *pceltfetched = reported.min(celt) };
            }

            if fetched == 0 {
                // Enumeration exhausted
                windows::Win32::Foundation::S_FALSE
            } else if reported >= celt {
                windows::Win32::Foundation::S_OK
            } else {
                windows::Win32::Foundation::S_FALSE
            }
        }
        fn Skip(&self, _celt: u32) -> windows::core::HRESULT {
            windows::Win32::Foundation::E_NOTIMPL
        }
        fn Reset(&self) -> windows::core::Result<()> {
            self.index.store(0, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        fn Clone(&self) -> windows::core::Result<IEnumString> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
    }

    /// Mock that returns only null entries forever.
    #[allow(clippy::ref_as_ptr, clippy::inline_always)]
    #[implement(IEnumString)]
    struct MockEnumStringNullOnly;

    impl IEnumString_Impl for MockEnumStringNullOnly_Impl {
        fn Next(
            &self,
            _celt: u32,
            _rgelt: *mut PWSTR,
            pceltfetched: *mut u32,
        ) -> windows::core::HRESULT {
            if !pceltfetched.is_null() {
                unsafe { *pceltfetched = 1 };
            }
            windows::Win32::Foundation::S_OK.into()
        }
        fn Skip(&self, _celt: u32) -> windows::core::HRESULT {
            windows::Win32::Foundation::E_NOTIMPL.into()
        }
        fn Reset(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn Clone(&self) -> windows::core::Result<IEnumString> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
    }

    /// Mock that populates one COM-owned string and then returns an error.
    #[allow(clippy::ref_as_ptr, clippy::inline_always)]
    #[implement(IEnumString)]
    struct MockEnumStringWithError;

    impl IEnumString_Impl for MockEnumStringWithError_Impl {
        fn Next(
            &self,
            _celt: u32,
            rgelt: *mut PWSTR,
            pceltfetched: *mut u32,
        ) -> windows::core::HRESULT {
            let value: Vec<u16> = "leaked-unless-cleaned"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(
                    value.len() * std::mem::size_of::<u16>(),
                )
            };
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), ptr as *mut u16, value.len());
                *rgelt = PWSTR(ptr as *mut u16);
                if !pceltfetched.is_null() {
                    *pceltfetched = 1;
                }
            }
            windows::Win32::Foundation::E_FAIL.into()
        }
        fn Skip(&self, _celt: u32) -> windows::core::HRESULT {
            windows::Win32::Foundation::E_NOTIMPL.into()
        }
        fn Reset(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn Clone(&self) -> windows::core::Result<IEnumString> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
    }

    /// OPC-BUG-001 regression: null PWSTR entries within the fetched range
    /// must be silently skipped, not yield E_POINTER.
    #[test]
    fn test_string_iterator_null_entries_skipped() {
        let items = vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()];

        let mock_enum: IEnumString = MockEnumStringWithNulls {
            items: items.clone(),
            index: std::sync::atomic::AtomicUsize::new(0),
            extra_nulls: 5, // Claim 5 extra items that are actually null
        }
        .into();

        let iter = StringIterator::new(mock_enum);

        let mut results = Vec::new();
        for item in iter {
            // No E_POINTER should leak through
            let value = item.expect("Expected OK value, got phantom error from null entry");
            results.push(value);
        }

        assert_eq!(
            results, items,
            "Only valid items should be yielded, nulls skipped"
        );
    }

    #[test]
    fn test_string_iterator_terminates_on_null_only_batches() {
        let mock_enum: IEnumString = MockEnumStringNullOnly.into();
        let mut iter = StringIterator::new(mock_enum);

        let error = iter
            .next()
            .expect("the iterator must report null-only non-progress")
            .expect_err("null-only batches must terminate the iterator");
        assert!(matches!(
            error,
            OpcError::BrowseNonProgress {
                iterator_type,
                browse_path,
                repeated_value,
                consecutive,
                yielded,
            } if iterator_type == "IEnumString"
                && browse_path == "<root>"
                && repeated_value == "<null PWSTR>"
                && consecutive == MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES
                && yielded == 0
        ));
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_string_iterator_cleans_populated_slots_after_com_error() {
        let mock_enum: IEnumString = MockEnumStringWithError.into();
        let mut iter = StringIterator::new(mock_enum);

        assert!(
            iter.next()
                .expect("the COM error must be returned")
                .is_err()
        );
        assert_eq!(iter.populated, 1);
        assert!(!iter.cache[0].is_null());

        iter.release_pending();
        assert_eq!(iter.populated, 0);
        assert!(iter.cache[0].is_null());
    }

    /// Verify iterator handles a fully empty enumeration (0 items, immediate S_FALSE).
    #[test]
    fn test_string_iterator_empty() {
        let mock_enum: IEnumString = MockEnumString {
            items: Vec::new(),
            index: std::sync::atomic::AtomicUsize::new(0),
        }
        .into();

        let iter = StringIterator::new(mock_enum);
        let results: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(results.is_empty(), "Empty iterator should yield no items");
    }

    #[test]
    fn test_fetched_count_over_capacity_is_rejected() {
        let error = validate_fetched_count(17, MAX_CACHE_SIZE, "IEnumGUID")
            .expect_err("an oversized COM count must be rejected");
        assert!(error.to_string().contains("17 entries"));
        assert!(error.to_string().contains("16-entry cache"));
    }
}
