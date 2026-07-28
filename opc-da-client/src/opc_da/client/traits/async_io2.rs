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

        // SAFETY: Calling COM interface method Read with valid handles and array pointers.
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

        // SAFETY: Calling COM interface method Write with valid handles, values, and array pointers.
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
        // SAFETY: Calling COM interface method Refresh2.
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
        // SAFETY: Calling COM interface method Cancel2.
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
        // SAFETY: Calling COM interface method SetEnable.
        unsafe { self.interface()?.SetEnable(enable).map_err(OpcError::from) }
    }

    /// Gets the current enable state of asynchronous I/O operations.
    ///
    /// # Returns
    /// `true` if async operations are enabled, `false` otherwise
    fn get_enable(&self) -> OpcResult<bool> {
        // SAFETY: Calling COM interface method GetEnable.
        unsafe {
            self.interface()?
                .GetEnable()
                .map(|v| v.as_bool())
                .map_err(OpcError::from)
        }
    }
}
