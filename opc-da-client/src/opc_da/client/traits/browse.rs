use crate::bindings::da::{
    IOPCBrowse, tagOPCBROWSEELEMENT, tagOPCBROWSEFILTER, tagOPCITEMPROPERTIES,
};
use crate::opc_da::com_utils::{LocalPointer, RemoteArray, RemotePointer};
use crate::opc_da::errors::{OpcError, OpcResult};

/// Server address space browsing functionality (OPC DA 3.0).
///
/// Provides methods to browse the hierarchical namespace of an OPC server
/// and retrieve item properties.
pub trait BrowseTrait {
    fn interface(&self) -> OpcResult<&IOPCBrowse>;

    /// Gets properties for specified items from the server.
    ///
    /// # Arguments
    /// * `item_ids` - Array of item identifiers to get properties for
    /// * `return_property_values` - If true, return actual property values; if false, only property metadata
    /// * `property_ids` - Specific property IDs to retrieve; empty array means all properties
    ///
    /// # Returns
    /// Array of item properties containing property values and/or metadata
    ///
    /// # Errors
    /// Returns E_INVALIDARG if item_ids is empty
    fn get_properties(
        &self,
        item_ids: &[String],
        return_property_values: bool,
        property_ids: &[u32],
    ) -> OpcResult<RemoteArray<tagOPCITEMPROPERTIES>> {
        if item_ids.is_empty() {
            return Err(OpcError::InvalidState("item_ids is empty".to_string()));
        }

        let item_ptrs: LocalPointer<Vec<Vec<u16>>> = LocalPointer::from(item_ids);
        let item_ptrs = item_ptrs.as_pcwstr_array();

        let mut results = RemoteArray::new(item_ids.len().try_into()?);

        // SAFETY: Calling COM interface method GetProperties with valid array pointers and handles.
        unsafe {
            self.interface()?.GetProperties(
                item_ids.len() as u32,
                item_ptrs.as_ptr(),
                return_property_values,
                property_ids,
                results.as_mut_ptr(),
            )?;
        }

        Ok(results)
    }

    /// Browses a single branch or leaf in the server's address space.
    ///
    /// # Arguments
    /// * `item_id` - Starting point for browsing (empty string for root)
    /// * `max_elements` - Maximum number of elements to return
    /// * `browse_filter` - Filter specifying what types of elements to return
    /// * `element_name_filter` - Filter string for element names (can contain wildcards)
    /// * `vendor_filter` - Vendor-specific filter string
    /// * `return_all_properties` - If true, return all available properties
    /// * `return_property_values` - If true, return property values; if false, only property metadata
    /// * `property_ids` - Specific property IDs to retrieve when return_all_properties is false
    ///
    /// # Returns
    /// Tuple containing:
    /// - Boolean indicating if more elements are available
    /// - Array of browse elements containing names and properties
    #[allow(clippy::too_many_arguments)]
    fn browse<S0, S1, S2, S3>(
        &self,
        item_id: Option<S0>,
        continuation_point: Option<S1>,
        max_elements: u32,
        browse_filter: tagOPCBROWSEFILTER,
        element_name_filter: Option<S2>,
        vendor_filter: Option<S3>,
        return_all_properties: bool,
        return_property_values: bool,
        property_ids: &[u32],
    ) -> OpcResult<(bool, Option<String>, RemoteArray<tagOPCBROWSEELEMENT>)>
    where
        S0: AsRef<str>,
        S1: AsRef<str>,
        S2: AsRef<str>,
        S3: AsRef<str>,
    {
        let item_id = LocalPointer::from(item_id.as_ref().map_or("", |value| value.as_ref()));
        let element_name_filter = LocalPointer::from(
            element_name_filter
                .as_ref()
                .map_or("", |value| value.as_ref()),
        );
        let vendor_filter =
            LocalPointer::from(vendor_filter.as_ref().map_or("", |value| value.as_ref()));
        let mut continuation_point =
            RemotePointer::from_option(continuation_point.as_ref().map(|v| v.as_ref()));
        let mut more_elements = false.into();
        let mut count = 0;
        let mut elements = RemoteArray::empty();

        // SAFETY: Calling COM interface method Browse with valid strings and output pointers.
        unsafe {
            self.interface()?.Browse(
                item_id.as_pcwstr(),
                continuation_point.as_mut_pwstr_ptr(),
                max_elements,
                browse_filter,
                element_name_filter.as_pcwstr(),
                vendor_filter.as_pcwstr(),
                return_all_properties,
                return_property_values,
                property_ids,
                &mut more_elements,
                &mut count,
                elements.as_mut_ptr(),
            )?;
        }

        if count > 0 {
            // SAFETY: Updating array length based on count returned by Browse.
            unsafe { elements.set_len(count) };
        }

        Ok((
            more_elements.into(),
            continuation_point.try_into()?,
            elements,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::da::{IOPCBrowse_Impl, OPC_BROWSE_FILTER_ALL};
    use std::sync::{Arc, Mutex};
    use windows::Win32::Foundation::E_NOTIMPL;
    use windows::core::{BOOL, PCWSTR, PWSTR, implement};

    #[derive(Debug, PartialEq, Eq)]
    struct BrowseCall {
        item_id_pointer_is_null: bool,
        item_id: Option<String>,
        continuation_pointer_is_null: bool,
        continuation: Option<String>,
        element_filter_pointer_is_null: bool,
        element_filter: Option<String>,
        vendor_filter_pointer_is_null: bool,
        vendor_filter: Option<String>,
        property_count: u32,
        property_pointer_is_null: bool,
        property_ids: Vec<u32>,
    }

    #[allow(clippy::ref_as_ptr, clippy::inline_always)]
    #[implement(IOPCBrowse)]
    struct BrowseRecorder {
        calls: Arc<Mutex<Vec<BrowseCall>>>,
    }

    impl IOPCBrowse_Impl for BrowseRecorder_Impl {
        fn GetProperties(
            &self,
            _dwitemcount: u32,
            _pszitemids: *const PCWSTR,
            _breturnpropertyvalues: BOOL,
            _dwpropertycount: u32,
            _pdwpropertyids: *const u32,
            _ppitemproperties: *mut *mut tagOPCITEMPROPERTIES,
        ) -> windows::core::Result<()> {
            Err(windows::core::Error::from_hresult(E_NOTIMPL))
        }

        fn Browse(
            &self,
            szitemid: &PCWSTR,
            pszcontinuationpoint: *mut PWSTR,
            _dwmaxelementsreturned: u32,
            _dwbrowsefilter: tagOPCBROWSEFILTER,
            szelementnamefilter: &PCWSTR,
            szvendorfilter: &PCWSTR,
            _breturnallproperties: BOOL,
            _breturnpropertyvalues: BOOL,
            dwpropertycount: u32,
            pdwpropertyids: *const u32,
            pbmoreelements: *mut BOOL,
            pdwcount: *mut u32,
            ppbrowseelements: *mut *mut tagOPCBROWSEELEMENT,
        ) -> windows::core::Result<()> {
            let continuation = if pszcontinuationpoint.is_null() {
                None
            } else {
                // SAFETY: The caller supplies a valid outer continuation pointer.
                let value = unsafe { *pszcontinuationpoint };
                if value.is_null() {
                    None
                } else {
                    // SAFETY: The continuation points to a NUL-terminated UTF-16 string.
                    Some(unsafe { value.to_string()? })
                }
            };
            let property_ids = if dwpropertycount == 0 {
                Vec::new()
            } else {
                // SAFETY: A non-zero count requires a valid array with that many IDs.
                unsafe {
                    std::slice::from_raw_parts(pdwpropertyids, dwpropertycount as usize).to_vec()
                }
            };
            self.calls.lock().unwrap().push(BrowseCall {
                item_id_pointer_is_null: szitemid.is_null(),
                item_id: pcwstr_to_string(szitemid)?,
                continuation_pointer_is_null: pszcontinuationpoint.is_null(),
                continuation,
                element_filter_pointer_is_null: szelementnamefilter.is_null(),
                element_filter: pcwstr_to_string(szelementnamefilter)?,
                vendor_filter_pointer_is_null: szvendorfilter.is_null(),
                vendor_filter: pcwstr_to_string(szvendorfilter)?,
                property_count: dwpropertycount,
                property_pointer_is_null: pdwpropertyids.is_null(),
                property_ids,
            });

            if !pbmoreelements.is_null() {
                // SAFETY: The caller supplied this optional output pointer.
                unsafe { *pbmoreelements = false.into() };
            }
            if !pdwcount.is_null() {
                // SAFETY: The caller supplied this optional output pointer.
                unsafe { *pdwcount = 0 };
            }
            if !ppbrowseelements.is_null() {
                // SAFETY: The caller supplied this optional output pointer.
                unsafe { *ppbrowseelements = core::ptr::null_mut() };
            }
            Ok(())
        }
    }

    fn pcwstr_to_string(value: &PCWSTR) -> windows::core::Result<Option<String>> {
        if value.is_null() {
            return Ok(None);
        }
        // SAFETY: The COM caller supplies a NUL-terminated UTF-16 string.
        Ok(Some(unsafe { value.to_string()? }))
    }

    struct TestBrowse(IOPCBrowse);

    impl BrowseTrait for TestBrowse {
        fn interface(&self) -> OpcResult<&IOPCBrowse> {
            Ok(&self.0)
        }
    }

    #[test]
    fn browse_marshals_absent_required_strings_as_non_null_empty_strings() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let interface: IOPCBrowse = BrowseRecorder {
            calls: Arc::clone(&calls),
        }
        .into();
        let browse = TestBrowse(interface);

        let (_, continuation, _) = browse
            .browse(
                None::<&str>,
                None::<&str>,
                100,
                OPC_BROWSE_FILTER_ALL,
                None::<&str>,
                None::<&str>,
                false,
                false,
                &[],
            )
            .unwrap();

        assert_eq!(continuation, None);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[BrowseCall {
                item_id_pointer_is_null: false,
                item_id: Some(String::new()),
                continuation_pointer_is_null: false,
                continuation: None,
                element_filter_pointer_is_null: false,
                element_filter: Some(String::new()),
                vendor_filter_pointer_is_null: false,
                vendor_filter: Some(String::new()),
                property_count: 0,
                property_pointer_is_null: false,
                property_ids: Vec::new(),
            }]
        );
    }

    #[test]
    fn browse_preserves_non_empty_strings_continuation_and_property_ids() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let interface: IOPCBrowse = BrowseRecorder {
            calls: Arc::clone(&calls),
        }
        .into();
        let browse = TestBrowse(interface);

        let (_, continuation, _) = browse
            .browse(
                Some("Channel.Device"),
                Some("resume-here"),
                25,
                OPC_BROWSE_FILTER_ALL,
                Some("Tag*"),
                Some("Vendor"),
                false,
                true,
                &[1, 5],
            )
            .unwrap();

        assert_eq!(continuation.as_deref(), Some("resume-here"));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[BrowseCall {
                item_id_pointer_is_null: false,
                item_id: Some("Channel.Device".to_string()),
                continuation_pointer_is_null: false,
                continuation: Some("resume-here".to_string()),
                element_filter_pointer_is_null: false,
                element_filter: Some("Tag*".to_string()),
                vendor_filter_pointer_is_null: false,
                vendor_filter: Some("Vendor".to_string()),
                property_count: 2,
                property_pointer_is_null: false,
                property_ids: vec![1, 5],
            }]
        );
    }
}
