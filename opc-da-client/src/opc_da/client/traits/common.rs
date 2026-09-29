use crate::opc_da::{
    com_utils::{LocalPointer, RemoteArray, RemotePointer},
    errors::{OpcError, OpcResult},
};

/// Common OPC server functionality trait.
///
/// Provides methods for locale management and error string retrieval.
/// This trait is implemented by all OPC DA servers to support basic
/// configuration and error handling capabilities.
pub trait CommonTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::comn::IOPCCommon>;

    /// Sets the locale ID for server string localization.
    ///
    /// # Arguments
    /// * `locale_id` - Windows LCID (Locale ID) value for the desired language
    ///
    /// # Returns
    /// Result indicating if the locale was successfully set
    fn set_locale_id(&self, locale_id: u32) -> OpcResult<()> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call, and `?`
        // propagates the HRESULT.
        unsafe { Ok(self.interface()?.SetLocaleID(locale_id)?) }
    }

    /// Gets the current locale ID used by the server.
    ///
    /// # Returns
    /// Windows LCID value representing the current locale
    fn get_locale_id(&self) -> OpcResult<u32> {
        // SAFETY: `self.interface()` keeps the COM object borrowed for this call, and `?`
        // propagates the HRESULT before returning the locale ID.
        unsafe { Ok(self.interface()?.GetLocaleID()?) }
    }

    /// Gets a list of locale IDs supported by the server.
    ///
    /// # Returns
    /// Array of Windows LCID values for supported locales
    fn query_available_locale_ids(&self) -> OpcResult<RemoteArray<u32>> {
        let mut locale_ids = RemoteArray::empty();

        // SAFETY: The borrowed interface stays live; both output pointers target fields in
        // the live `RemoteArray`. `?` checks the HRESULT before returning the array.
        unsafe {
            self.interface()?
                .QueryAvailableLocaleIDs(locale_ids.as_mut_len_ptr(), locale_ids.as_mut_ptr())?;
        }

        Ok(locale_ids)
    }

    /// Gets a localized error description string.
    ///
    /// # Arguments
    /// * `error` - HRESULT error code to get description for
    ///
    /// # Returns
    /// Localized error message string in current locale
    fn get_error_string(&self, error: windows::core::HRESULT) -> OpcResult<String> {
        // SAFETY: The borrowed interface stays live for the call, and `?` checks the HRESULT
        // before the returned COM-owned string is wrapped.
        let output = unsafe { self.interface()?.GetErrorString(error)? };

        RemotePointer::from(output)
            .try_into()
            .map_err(OpcError::from)
    }

    /// Sets a client name for server identification.
    ///
    /// # Arguments
    /// * `name` - Client application name or description
    ///
    /// # Returns
    /// Result indicating if the client name was successfully set
    fn set_client_name(&self, name: &str) -> OpcResult<()> {
        let name = LocalPointer::from(name);
        // SAFETY: `name` owns a NUL-terminated UTF-16 buffer through the call; the borrowed
        // interface stays live, and `?` checks the HRESULT.
        unsafe { Ok(self.interface()?.SetClientName(name.as_pcwstr())?) }
    }
}
