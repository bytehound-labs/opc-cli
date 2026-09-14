use crate::opc_da::{
    com_utils::{RemoteArray, clear_variant},
    errors::{OpcError, OpcResult},
    typedefs::{ItemHandle, OwnedItemVqt},
};
use windows::Win32::System::Variant::VARIANT;

/// Synchronous I/O functionality (OPC DA 3.0).
///
/// Provides enhanced synchronous read/write operations with support for
/// quality, timestamp, and maximum age constraints.
pub trait SyncIo2Trait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO2>;

    /// Reads values with maximum age constraint.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `max_age` - Maximum age constraints for each item in milliseconds
    ///
    /// # Returns
    /// Tuple containing:
    /// - Array of item values
    /// - Array of quality values
    /// - Array of timestamps
    /// - Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    #[allow(clippy::type_complexity)]
    fn read_max_age(
        &self,
        server_handles: &[ItemHandle],
        max_age: &[u32],
    ) -> OpcResult<(
        RemoteArray<VARIANT>,
        RemoteArray<u16>,
        RemoteArray<windows::Win32::Foundation::FILETIME>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        if server_handles.len() != max_age.len() {
            return Err(OpcError::InvalidState(
                "server_handles and max_age must have the same length".to_string(),
            ));
        }

        if server_handles.is_empty() {
            return Err(OpcError::InvalidState(
                "server_handles cannot be empty".to_string(),
            ));
        }

        let len = server_handles.len().try_into()?;

        let mut values = RemoteArray::new_with_cleanup(len, clear_variant);
        let mut qualities = RemoteArray::new(len);
        let mut timestamps = RemoteArray::new(len);
        let mut errors = RemoteArray::new(len);

        // SAFETY: Calling COM interface method ReadMaxAge with valid pointers and output arrays.
        unsafe {
            self.interface()?.ReadMaxAge(
                len,
                server_handles.as_ptr() as *const u32,
                max_age.as_ptr(),
                values.as_mut_ptr(),
                qualities.as_mut_ptr(),
                timestamps.as_mut_ptr(),
                errors.as_mut_ptr(),
            )?;
        }
        values.validate_output("IOPCSyncIO2::ReadMaxAge values")?;
        qualities.validate_output("IOPCSyncIO2::ReadMaxAge qualities")?;
        timestamps.validate_output("IOPCSyncIO2::ReadMaxAge timestamps")?;
        errors.validate_output("IOPCSyncIO2::ReadMaxAge errors")?;

        Ok((values, qualities, timestamps, errors))
    }

    /// Writes values with quality and timestamp information.
    ///
    /// # Arguments
    /// * `server_handles` - Array of server item handles
    /// * `values` - Array of value-quality-timestamp structures
    ///
    /// # Returns
    /// Array of per-item error codes
    ///
    /// # Errors
    /// Returns E_INVALIDARG if arrays are empty or have different lengths
    fn write_vqt(
        &self,
        server_handles: &[ItemHandle],
        values: &[OwnedItemVqt],
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

        // SAFETY: Calling COM interface method WriteVQT with valid handle and VQT array pointers.
        unsafe {
            self.interface()?.WriteVQT(
                len,
                server_handles.as_ptr() as *const u32,
                OwnedItemVqt::as_native_slice(values).as_ptr(),
                errors.as_mut_ptr(),
            )?;
        }
        errors.validate_output("IOPCSyncIO2::WriteVQT errors")?;

        Ok(errors)
    }
}
