use crate::opc_da::{
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
};

/// Item deadband management functionality (OPC DA 3.0).
///
/// Provides methods to manage per-item deadband values. Deadband settings
/// control the minimum value change required before the server reports
/// a data change to the client.
pub trait ItemDeadbandMgtTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemDeadbandMgt>;

    /// Sets deadband values for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `deadbands` - Array of deadband percentages (0.0 to 100.0)
    ///
    /// # Returns
    /// Array of HRESULT values indicating per-item status
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays have different lengths
    fn set_item_deadband(
        &self,
        server_handles: &[u32],
        deadbands: &[f32],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.len() != deadbands.len() {
            return Err(OpcError::InvalidState(
                "server_handles and deadbands must have the same length".to_string(),
            ));
        }

        // Validate deadband values (0.0 to 100.0)
        if deadbands.iter().any(|&v| !(0.0..=100.0).contains(&v)) {
            return Err(OpcError::InvalidState(
                "deadband values must be between 0.0 and 100.0".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method SetItemDeadband with valid pointers and array lengths.
        unsafe {
            self.interface()?.SetItemDeadband(
                len,
                server_handles.as_ptr(),
                deadbands.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Gets current deadband values for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of deadband percentages (0.0 to 100.0)
    /// - Array of HRESULT values indicating per-item status
    fn get_item_deadband(
        &self,
        server_handles: &[u32],
    ) -> OpcResult<(RemoteArray<f32>, RemoteArray<windows::core::HRESULT>)> {
        let len = server_handles.len().try_into()?;

        let mut errors = RemoteArray::new(len);
        let mut deadbands = RemoteArray::new(len);

        // SAFETY: Calling COM interface method GetItemDeadband with valid pointers and array lengths.
        unsafe {
            self.interface()?.GetItemDeadband(
                len,
                server_handles.as_ptr(),
                deadbands.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((deadbands, errors))
    }

    /// Removes deadband settings for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Array of HRESULT values indicating per-item status
    fn clear_item_deadband(
        &self,
        server_handles: &[u32],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        let len = server_handles.len().try_into()?;

        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method ClearItemDeadband with valid pointers and array lengths.
        unsafe {
            self.interface()?.ClearItemDeadband(
                len,
                server_handles.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }
}
