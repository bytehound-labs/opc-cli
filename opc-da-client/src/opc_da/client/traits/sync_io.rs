use crate::opc_da::{
    com_utils::RemoteArray,
    errors::{OpcError, OpcResult},
    typedefs::ItemHandle,
};
use windows::Win32::System::Variant::VARIANT;

/// Synchronous I/O functionality (OPC DA 1.0).
///
/// Provides methods for basic synchronous read/write operations
/// with direct server communication.
pub trait SyncIoTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO>;

    /// Reads values synchronously from items.
    ///
    /// # Arguments
    /// * `source` - Whether to read from cache or device
    /// * `server_handles` - Array of server item handles
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of item states (value, quality, timestamp)
    /// - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if server_handles is empty
    fn read(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(
        RemoteArray<crate::bindings::da::tagOPCITEMSTATE>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut item_values = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method Read with valid server handle and output array pointers.
        unsafe {
            self.interface()?.Read(
                source,
                len,
                server_handles.as_ptr() as *const u32,
                item_values.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok((item_values, errors))
    }

    /// Writes values synchronously to items.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `values` - Array of values to write
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
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

        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method Write with valid server handle and value array pointers.
        unsafe {
            self.interface()?.Write(
                len,
                server_handles.as_ptr() as *const u32,
                values.as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }

        Ok(errors)
    }
}
