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
    last_value: Option<String>,
    consecutive: usize,
    yielded: usize,
}

impl StringIterator {
    pub fn new(inner: windows::Win32::System::Com::IEnumString) -> Self {
        Self {
            inner,
            cache: Box::new([windows::core::PWSTR::null(); STRING_CACHE_SIZE]),
            index: STRING_CACHE_SIZE as u32,
            count: 0,
            done: false,
            last_value: None,
            consecutive: 0,
            yielded: 0,
        }
    }
}

impl Drop for StringIterator {
    fn drop(&mut self) {
        let cache_len = self.cache.len();
        let start = usize::try_from(self.index.min(self.count)).unwrap_or(0);
        let end = usize::try_from(self.count).unwrap_or(self.cache.len());
        for slot in self.cache[start..end.min(cache_len)].iter_mut() {
            let pwstr = std::mem::replace(slot, windows::core::PWSTR::null());
            if !pwstr.is_null() {
                drop(RemotePointer::from(pwstr));
            }
        }
    }
}

impl Iterator for StringIterator {
    type Item = OpcResult<String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }

            if self.index >= self.count {
                // Zero the cache to prevent stale freed pointers (OPC-BUG-001)
                let started = Instant::now();
                tracing::info!(celt = self.cache.len(), "Starting IEnumString::Next");
                self.cache.fill(windows::core::PWSTR::null());

                // SAFETY: Calling IEnumString::Next COM interface method with valid mutable cache slice and count pointer.
                let code = unsafe {
                    self.inner
                        .Next(self.cache.as_mut_slice(), Some(&mut self.count))
                };

                tracing::info!(
                    hresult = format_args!("{:#010X}", code.0),
                    celt = self.cache.len(),
                    fetched = self.count,
                    elapsed_ms = started.elapsed().as_millis(),
                    "IEnumString::Next returned"
                );
                tracing::debug!(
                    hresult = format_args!("{:#010X}", code.0),
                    celt = self.cache.len(),
                    fetched = self.count,
                    "StringIterator::Next completed"
                );

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
                    let null_count = self.cache[..self.count as usize]
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
            assert_eq!(iter.next(), Some(Ok("\u{1}".to_string())));
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
        assert_eq!(iter.next(), None);
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
