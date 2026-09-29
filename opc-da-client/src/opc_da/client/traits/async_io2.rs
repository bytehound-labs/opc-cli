use crate::opc_da::{
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
    typedefs::ItemHandle,
};

/// Asynchronous I/O functionality (OPC DA 2.0).
///
/// Provides enhanced asynchronous read/write operations without requiring
/// connection point callbacks. This trait extends the functionality of
/// AsyncIoTrait with improved error handling and control mechanisms.
pub trait AsyncIo2Trait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO2>;

    /// Initiates an asynchronous read operation.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles to read
    /// * `transaction_id` - Client-provided transaction identifier
    ///
    /// # Returns
    /// Tuple containing (cancel_id, error_array) where:
    /// - cancel_id: Identifier used to cancel the operation
    /// - error_array: Array of HRESULT values indicating per-item status
    fn read(
        &self,
        server_handles: &[ItemHandle],
        transaction_id: u32,
    ) -> OpcResult<(u32, RemoteArray<windows::core::HRESULT>)> {
        let len = server_handles
            .len()
            .try_into()
            .map_err(crate::opc_da::errors::OpcError::from)?;

        let mut cancel_id = 0;
        let mut errors = RemoteArray::new(len);

        // SAFETY: `len` comes from the live handle slice; that slice, the local cancel ID,
        // SAFETY: and the `RemoteArray` output slot remain valid through the borrowed-interface call.
        // SAFETY: `?` checks the HRESULT before returning the outputs.
        unsafe {
            self.interface()?.Read(
                len,
                server_handles.as_ptr() as *const u32,
                transaction_id,
                &mut cancel_id,
                errors.as_mut_ptr(),
            )?;
        }

        Ok((cancel_id, errors))
    }

    /// Initiates an asynchronous write operation.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles to write
    /// * `values` - Array of VARIANT values to write
    /// * `transaction_id` - Client-provided transaction identifier
    ///
    /// # Returns
    /// Tuple containing (cancel_id, error_array) where:
    /// - cancel_id: Identifier used to cancel the operation
    /// - error_array: Array of HRESULT values indicating per-item status
    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[windows::Win32::System::Variant::VARIANT],
        transaction_id: u32,
    ) -> OpcResult<(u32, RemoteArray<windows::core::HRESULT>)> {
        let len = server_handles
            .len()
            .try_into()
            .map_err(crate::opc_da::errors::OpcError::from)?;

        let mut cancel_id = 0;
        let mut errors = RemoteArray::new(len);

        // SAFETY: COM reads `len` entries from both slices, but `len` is taken only from
        // SAFETY: `server_handles`; `values` must also have at least `len` entries (not checked here).
        // SAFETY: The slices and output slots remain live through the borrowed-interface call.
        unsafe {
            self.interface()?.Write(
                len,
                server_handles.as_ptr() as *const u32,
                values.as_ptr(),
                transaction_id,
                &mut cancel_id,
                errors.as_mut_ptr(),
            )?;
        }

        Ok((cancel_id, errors))
    }

    /// Refreshes all active items from the specified source.
    ///
    /// # Arguments
    /// * `source` - Data source (cache or device)
    /// * `transaction_id` - Client-provided transaction identifier
    ///
    /// # Returns
    /// Cancel ID that can be used to cancel the operation
    fn refresh2(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        transaction_id: u32,
    ) -> OpcResult<u32> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call; the
        // SAFETY: returned HRESULT is converted to `OpcError` before it is returned.
        unsafe {
            self.interface()?
                .Refresh2(source, transaction_id)
                .map_err(OpcError::from)
        }
    }

    /// Cancels a pending asynchronous operation.
    ///
    /// # Arguments
    /// * `cancel_id` - Cancel ID returned from read/write operations
    ///
    /// # Returns
    /// `Ok(())` if the operation was successfully canceled
    fn cancel2(&self, cancel_id: u32) -> OpcResult<()> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call; the
        // SAFETY: returned HRESULT is converted to `OpcError` before it is returned.
        unsafe { self.interface()?.Cancel2(cancel_id).map_err(OpcError::from) }
    }

    /// Enables or disables asynchronous I/O operations.
    ///
    /// # Arguments
    /// * `enable` - `true` to enable async operations, `false` to disable
    ///
    /// # Returns
    /// `Ok(())` if the enable state was successfully changed
    fn set_enable(&self, enable: bool) -> OpcResult<()> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call; the
        // SAFETY: returned HRESULT is converted to `OpcError` before it is returned.
        unsafe { self.interface()?.SetEnable(enable).map_err(OpcError::from) }
    }

    /// Gets the current enable state of asynchronous I/O operations.
    ///
    /// # Returns
    /// `true` if async operations are enabled, `false` otherwise
    fn get_enable(&self) -> OpcResult<bool> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call, and the
        // SAFETY: returned HRESULT is converted before the BOOL is read.
        unsafe {
            self.interface()?
                .GetEnable()
                .map(|v| v.as_bool())
                .map_err(OpcError::from)
        }
    }
}
