use crate::bindings::da::IOPCItemProperties;
use crate::opc_da::{
    com_utils::{LocalPointer, RemoteArray},
    errors::{OpcError, OpcResult},
};

/// Item properties management functionality.
///
/// Provides methods to query and retrieve item property information from
/// the OPC server. Properties include metadata such as engineering units,
/// descriptions, and other vendor-specific attributes.
pub trait ItemPropertiesTrait {
    fn interface(&self) -> OpcResult<&IOPCItemProperties>;

    /// Queries available properties for a specific item.
    ///
    /// # Arguments
    /// * `item_id` - Fully qualified item ID
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of property IDs
    /// - Array of property descriptions
    /// - Array of property data types (VT_*)
    ///
    /// # Errors
    /// Returns E_INVALIDARG if item_id is empty
    fn query_available_properties(
        &self,
        item_id: &str,
    ) -> OpcResult<(
        RemoteArray<u32>,                  // property IDs
        RemoteArray<windows::core::PWSTR>, // descriptions
        RemoteArray<u16>,                  // datatypes
    )> {
        if item_id.is_empty() {
            return Err(OpcError::InvalidState("item_id is empty".to_string()));
        }

        let item_id = LocalPointer::from(item_id);

        let mut count = 0;
        let mut property_ids = RemoteArray::new(0);
        let mut descriptions = RemoteArray::new(0);
        let mut datatypes = RemoteArray::new(0);

        // SAFETY: `item_id` owns a NUL-terminated UTF-16 buffer through the call; COM fills
        // the count and three live output slots. `?` checks the HRESULT before `count` is used.
        unsafe {
            self.interface()?.QueryAvailableProperties(
                item_id.as_pcwstr(),
                &mut count,
                property_ids.as_mut_ptr(),
                descriptions.as_mut_ptr(),
                datatypes.as_mut_ptr(),
            )?;
        }

        if count > 0 {
            // SAFETY: The successful `QueryAvailableProperties` call returned these three
            // COM arrays with `count` initialized elements, matching the OPC DA contract.
            unsafe {
                property_ids.set_len(count);
                descriptions.set_len(count);
                datatypes.set_len(count);
            }
        }

        Ok((property_ids, descriptions, datatypes))
    }

    /// Gets property values for a specific item.
    ///
    /// # Arguments
    /// * `item_id` - Fully qualified item ID
    /// * `property_ids` - Array of property IDs to retrieve
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of property values as VARIANTs
    /// - Array of per-property error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if property_ids is empty
    fn get_item_properties(
        &self,
        item_id: &str,
        property_ids: &[u32],
    ) -> OpcResult<(
        RemoteArray<windows::Win32::System::Variant::VARIANT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if property_ids.is_empty() {
            return Err(OpcError::InvalidState("property_ids is empty".to_string()));
        }

        let item_id = LocalPointer::from(item_id);

        let mut values = RemoteArray::new(property_ids.len().try_into()?);
        let mut errors = RemoteArray::new(property_ids.len().try_into()?);

        // SAFETY: `item_id` and the `property_ids` slice stay valid for the call; `len` is
        // the property slice length, and both output slots are live. `?` checks the HRESULT.
        unsafe {
            self.interface()?.GetItemProperties(
                item_id.as_pcwstr(),
                property_ids.len() as u32,
                property_ids.as_ptr(),
                values.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((values, errors))
    }

    /// Looks up item IDs for properties that are themselves OPC items.
    ///
    /// # Arguments
    /// * `item_id` - Base item ID to look up properties for
    /// * `property_ids` - Array of property IDs to look up
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of property-specific item IDs
    /// - Array of per-property error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if property_ids is empty
    fn lookup_item_ids(
        &self,
        item_id: &str,
        property_ids: &[u32],
    ) -> OpcResult<(
        RemoteArray<windows::core::PWSTR>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if property_ids.is_empty() {
            return Err(OpcError::InvalidState("property_ids is empty".to_string()));
        }

        let item_id = LocalPointer::from(item_id);

        let mut new_item_ids = RemoteArray::new(property_ids.len().try_into()?);
        let mut errors = RemoteArray::new(property_ids.len().try_into()?);

        // SAFETY: `item_id` and the `property_ids` slice stay valid for the call; `len` is
        // the property slice length, and both output slots are live. `?` checks the HRESULT.
        unsafe {
            self.interface()?.LookupItemIDs(
                item_id.as_pcwstr(),
                property_ids.len().try_into()?,
                property_ids.as_ptr(),
                new_item_ids.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((new_item_ids, errors))
    }
}
