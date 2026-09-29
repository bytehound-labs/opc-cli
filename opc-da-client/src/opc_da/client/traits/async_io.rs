use crate::opc_da::{
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
    typedefs::ItemHandle,
};
use windows::Win32::System::Variant::VARIANT;

/// Asynchronous I/O functionality (OPC DA 1.0).
///
/// Provides basic asynchronous read/write operations using connection point callbacks.
/// This is the original asynchronous interface defined in OPC DA 1.0.
pub trait AsyncIoTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO>;

    /// Reads values asynchronously from the server.
    ///
    /// # Arguments
    /// * `connection` - Connection point cookie for receiving callbacks
    /// * `source` - Specifies whether to read from cache or device
    /// * `server_handles` - Array of server item handles to read
    ///
    /// # Returns
    /// * `transaction_id` - Identifies this operation in callbacks
    /// * `errors` - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if server_handles is empty
    fn read(
        &self,
        connection: u32,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(u32, RemoteArray<windows::core::HRESULT>)> {
        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut transaction_id = 0;
        let mut errors = RemoteArray::new(len);

        // SAFETY: The handle slice has `len` entries; its storage, `transaction_id`, and
        // the `RemoteArray` output slot stay live through this call on the borrowed interface.
        // `?` checks the HRESULT before the outputs are returned.
        unsafe {
            self.interface()?.Read(
                connection,
                source,
                len,
                server_handles.as_ptr() as *const u32,
                &mut transaction_id,
                errors.as_mut_ptr(),
            )?;
        }

        Ok((transaction_id, errors))
    }

    /// Writes values asynchronously to the server.
    ///
    /// # Arguments
    /// * `connection` - Connection point cookie for receiving callbacks
    /// * `server_handles` - Array of server item handles to write
    /// * `values` - Array of values to write
    ///
    /// # Returns
    /// * `transaction_id` - Identifies this operation in callbacks
    /// * `errors` - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    fn write(
        &self,
        connection: u32,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<(u32, RemoteArray<windows::core::HRESULT>)> {
        if server_handles.len() != values.len() {
            return Err(OpcError::InvalidState(
                "server_handles and values must have the same length".to_string(),
            ));
        }

        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut transaction_id = 0;
        let mut errors = RemoteArray::new(len);

        // SAFETY: The equal-length handle and value slices both contain `len` entries and
        // remain borrowed through the call; the transaction ID and output slot are live.
        // `?` checks the HRESULT before the outputs are returned.
        unsafe {
            self.interface()?.Write(
                connection,
                len,
                server_handles.as_ptr() as *const u32,
                values.as_ptr(),
                &mut transaction_id,
                errors.as_mut_ptr(),
            )?;
        }

        Ok((transaction_id, errors))
    }

    /// Refreshes all active items asynchronously.
    ///
    /// # Arguments
    /// * `connection` - Connection point cookie for receiving callbacks
    /// * `source` - Specifies whether to refresh from cache or device
    ///
    /// # Returns
    /// Transaction ID for identifying the operation in callbacks
    fn refresh(
        &self,
        connection: u32,
        source: crate::bindings::da::tagOPCDATASOURCE,
    ) -> OpcResult<u32> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call, and `?`
        // propagates a failed HRESULT before returning the cancel ID.
        unsafe { Ok(self.interface()?.Refresh(connection, source)?) }
    }

    /// Cancels an outstanding asynchronous operation.
    ///
    /// # Arguments
    /// * `transaction_id` - ID of the operation to cancel
    ///
    /// # Returns
    /// Result indicating success or failure of cancel request
    fn cancel(&self, transaction_id: u32) -> OpcResult<()> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call, and `?`
        // propagates a failed HRESULT.
        unsafe { Ok(self.interface()?.Cancel(transaction_id)?) }
    }
}
