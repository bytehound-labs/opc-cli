use crate::opc_da::{
    client::ItemAttributeIterator,
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
    typedefs::ItemHandle,
};
use windows::core::Interface as _;

/// Item management functionality.
///
/// Provides methods to manage OPC items within a group, including adding,
/// removing, and modifying item properties such as active state, client
/// handles, and data types.
pub trait ItemMgtTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemMgt>;

    /// Adds items to the group.
    ///
    /// # Arguments
    /// * `items` - Array of item definitions containing item IDs and requested properties
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of item results with server handles and canonical data type
    /// - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if items array is empty
    fn add_items(
        &self,
        items: &[crate::bindings::da::tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<crate::bindings::da::tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if items.is_empty() {
            return Err(OpcError::InvalidState("items cannot be empty".to_string()));
        }

        let len = items.len().try_into()?;
        tracing::debug!(
            item_count = len,
            "Adding items to OPC group natively via IOPCItemMgt"
        );
        let mut results = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method AddItems with valid item definition pointers and output arrays.
        unsafe {
            self.interface()?.AddItems(
                len,
                items.as_ptr(),
                results.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((results, errors))
    }

    /// Validates item definitions without adding them to the group.
    ///
    /// # Arguments
    /// * `items` - Array of item definitions to validate
    /// * `blob_update` - Whether to validate blob update capability
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of item results with access rights and canonical data type
    /// - Array of per-item error codes
    fn validate_items(
        &self,
        items: &[crate::bindings::da::tagOPCITEMDEF],
        blob_update: bool,
    ) -> OpcResult<(
        RemoteArray<crate::bindings::da::tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if items.is_empty() {
            return Err(OpcError::InvalidState("items cannot be empty".to_string()));
        }

        let len = items.len().try_into()?;
        let mut results = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method ValidateItems with valid item definition pointers and output arrays.
        unsafe {
            self.interface()?.ValidateItems(
                len,
                items.as_ptr(),
                blob_update,
                results.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((results, errors))
    }

    /// Removes items from the group.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server handles for items to remove
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if server_handles is empty
    fn remove_items(
        &self,
        server_handles: &[ItemHandle],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;
        tracing::debug!(
            item_count = len,
            "Removing items from OPC group natively via IOPCItemMgt"
        );
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method RemoveItems with valid server handles pointer.
        unsafe {
            self.interface()?.RemoveItems(
                len,
                server_handles.as_ptr() as *const u32,
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Sets the active state of items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server handles
    /// * `active` - True to activate items, false to deactivate
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if server_handles is empty
    fn set_active_state(
        &self,
        server_handles: &[ItemHandle],
        active: bool,
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method SetActiveState with valid server handles pointer.
        unsafe {
            self.interface()?.SetActiveState(
                len,
                server_handles.as_ptr() as *const u32,
                active,
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Sets client handles for items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server handles
    /// * `client_handles` - Array of new client handles
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    fn set_client_handles(
        &self,
        server_handles: &[ItemHandle],
        client_handles: &[ItemHandle],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.len() != client_handles.len() {
            return Err(OpcError::InvalidState(
                "server_handles and client_handles must have the same length".to_string(),
            ));
        }

        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method SetClientHandles with valid handle pointers.
        unsafe {
            self.interface()?.SetClientHandles(
                len,
                server_handles.as_ptr() as *const u32,
                client_handles.as_ptr() as *const u32,
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Sets requested data types for items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server handles
    /// * `requested_datatypes` - Array of VT_* data types
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    fn set_datatypes(
        &self,
        server_handles: &[ItemHandle],
        requested_datatypes: &[u16],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.len() != requested_datatypes.len() {
            return Err(OpcError::InvalidState(
                "server_handles and requested_datatypes must have the same length".to_string(),
            ));
        }

        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method SetDatatypes with valid datatype array pointer.
        unsafe {
            self.interface()?.SetDatatypes(
                len,
                server_handles.as_ptr() as *const u32,
                requested_datatypes.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Creates an enumerator for item management.
    ///
    /// # Arguments
    /// * `id` - Interface ID specifying the type of enumerator
    ///
    /// # Returns
    /// Enumerator interface for iterating through items
    fn create_enumerator(&self) -> OpcResult<ItemAttributeIterator> {
        // SAFETY: Calling COM interface method CreateEnumerator.
        let enumerator = unsafe {
            self.interface()?.CreateEnumerator(
                &<crate::bindings::da::IEnumOPCItemAttributes as windows_core::Interface>::IID,
            )?
        };

        Ok(ItemAttributeIterator::new(enumerator.cast()?))
    }
}

// ...existing code...
