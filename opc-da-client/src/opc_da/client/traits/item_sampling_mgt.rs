use crate::opc_da::{
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
};
use windows_core::BOOL;

/// Item sampling management functionality (OPC DA 3.0).
///
/// Provides methods to control sampling rates and buffering behavior
/// for individual items in an OPC group.
pub trait ItemSamplingMgtTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemSamplingMgt>;

    /// Sets sampling rates for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `sampling_rates` - Array of requested sampling rates in milliseconds
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of actual sampling rates set by server
    /// - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays have different lengths
    fn set_item_sampling_rate(
        &self,
        server_handles: &[u32],
        sampling_rates: &[u32],
    ) -> OpcResult<(RemoteArray<u32>, RemoteArray<windows::core::HRESULT>)> {
        if server_handles.len() != sampling_rates.len() {
            return Err(OpcError::InvalidState(
                "server_handles and sampling_rates must have the same length".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut revised_rates = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method SetItemSamplingRate with valid pointers and array lengths.
        unsafe {
            self.interface()?.SetItemSamplingRate(
                len,
                server_handles.as_ptr(),
                sampling_rates.as_ptr(),
                revised_rates.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((revised_rates, errors))
    }

    /// Gets current sampling rates for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of current sampling rates in milliseconds
    /// - Array of per-item error codes
    fn get_item_sampling_rate(
        &self,
        server_handles: &[u32],
    ) -> OpcResult<(RemoteArray<u32>, RemoteArray<windows::core::HRESULT>)> {
        let len = server_handles.len().try_into()?;

        let mut sampling_rates = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method GetItemSamplingRate with valid pointers and array lengths.
        unsafe {
            self.interface()?.GetItemSamplingRate(
                len,
                server_handles.as_ptr(),
                sampling_rates.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((sampling_rates, errors))
    }

    /// Removes custom sampling rates for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Array of per-item error codes
    fn clear_item_sampling_rate(
        &self,
        server_handles: &[u32],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        let len = server_handles.len().try_into()?;

        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method ClearItemSamplingRate with valid pointers and array lengths.
        unsafe {
            self.interface()?.ClearItemSamplingRate(
                len,
                server_handles.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Enables or disables data buffering for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `enable` - Array of boolean values to enable/disable buffering
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays have different lengths
    fn set_item_buffer_enable(
        &self,
        server_handles: &[u32],
        enable: &[bool],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        if server_handles.len() != enable.len() {
            return Err(OpcError::InvalidState(
                "server_handles and enable must have the same length".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut errors = RemoteArray::new(len);
        let enable_bool: Vec<BOOL> = enable.iter().map(|&v| BOOL::from(v)).collect();

        // SAFETY: Calling COM interface method SetItemBufferEnable with valid pointers and array lengths.
        unsafe {
            self.interface()?.SetItemBufferEnable(
                len,
                server_handles.as_ptr(),
                enable_bool.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }

    /// Gets current buffer enable states for specified items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of current buffer enable states
    /// - Array of per-item error codes
    fn get_item_buffer_enable(
        &self,
        server_handles: &[u32],
    ) -> OpcResult<(
        RemoteArray<windows_core::BOOL>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        let len = server_handles.len().try_into()?;

        let mut enable = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method GetItemBufferEnable with valid pointers and array lengths.
        unsafe {
            self.interface()?.GetItemBufferEnable(
                len,
                server_handles.as_ptr(),
                enable.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((enable, errors))
    }
}
