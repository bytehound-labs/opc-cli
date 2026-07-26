use crate::opc_da::errors::{OpcError, OpcResult};
use windows::Win32::System::Com::{FORMATETC, STGMEDIUM};

/// Data transfer functionality using COM's structured storage.
///
/// Provides methods to transfer data between client and server using
/// structured storage formats and advisory connections.
pub trait DataObjectTrait {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IDataObject>;

    /// Gets data from the object in the specified format.
    ///
    /// # Arguments
    /// * `format` - Format specification including clipboard format and storage medium
    ///
    /// # Returns
    /// Storage medium containing the requested data
    fn get_data(&self, format: &FORMATETC) -> OpcResult<STGMEDIUM> {
        // SAFETY: Calling COM method GetData with valid formatetc reference.
        unsafe { Ok(self.interface()?.GetData(format)?) }
    }

    /// Gets data in place using the specified format.
    ///
    /// # Arguments
    /// * `format` - Format specification including clipboard format and storage medium
    ///
    /// # Returns
    /// Storage medium updated with the requested data
    fn get_data_here(&self, format: &FORMATETC) -> OpcResult<STGMEDIUM> {
        let mut output = STGMEDIUM::default();
        // SAFETY: Calling COM method GetDataHere with output stgmedium reference.
        unsafe { self.interface()?.GetDataHere(format, &mut output)? };
        Ok(output)
    }

    /// Tests if data is available in the specified format.
    ///
    /// # Arguments
    /// * `format` - Format specification to test for availability
    ///
    /// # Returns
    /// Ok(()) if the format is supported, error otherwise
    fn query_get_data(&self, format: &FORMATETC) -> OpcResult<()> {
        // SAFETY: Calling COM method QueryGetData.
        unsafe {
            self.interface()?
                .QueryGetData(format)
                .ok()
                .map_err(OpcError::from)
        }
    }

    /// Gets the canonical format equivalent to the specified format.
    ///
    /// # Arguments
    /// * `format_in` - Format specification to convert
    ///
    /// # Returns
    /// Canonical format specification
    fn get_canonical_format(&self, format_in: &FORMATETC) -> OpcResult<FORMATETC> {
        let mut output = FORMATETC::default();
        // SAFETY: Calling COM method GetCanonicalFormatEtc.
        unsafe {
            self.interface()?
                .GetCanonicalFormatEtc(format_in, &mut output)
        }
        .ok()?;

        Ok(output)
    }

    /// Sets data in the specified format.
    ///
    /// # Arguments
    /// * `format` - Format specification for the data
    /// * `medium` - Storage medium containing the data
    /// * `release` - If true, the object takes ownership of medium
    ///
    /// # Returns
    /// Ok(()) if data was set successfully
    fn set_data(&self, format: &FORMATETC, medium: &STGMEDIUM, release: bool) -> OpcResult<()> {
        // SAFETY: Calling COM method SetData.
        unsafe { Ok(self.interface()?.SetData(format, medium, release)?) }
    }

    /// Enumerates available data formats.
    ///
    /// # Arguments
    /// * `direction` - Direction of data flow (DATADIR_GET = 1, DATADIR_SET = 2)
    ///
    /// # Returns
    /// Enumerator for available format specifications
    fn enumerate_formats(
        &self,
        direction: u32,
    ) -> OpcResult<windows::Win32::System::Com::IEnumFORMATETC> {
        // SAFETY: Calling COM method EnumFormatEtc.
        unsafe { Ok(self.interface()?.EnumFormatEtc(direction)?) }
    }

    /// Establishes an advisory connection for data change notifications.
    ///
    /// # Arguments
    /// * `format` - Format specification to monitor
    /// * `advf` - Advisory flags controlling notification behavior
    /// * `sink` - Sink interface to receive notifications
    ///
    /// # Returns
    /// Connection token for the advisory connection
    fn dadvise(
        &self,
        format: &FORMATETC,
        advf: u32,
        sink: &windows::Win32::System::Com::IAdviseSink,
    ) -> OpcResult<u32> {
        // SAFETY: Calling COM method DAdvise.
        unsafe { Ok(self.interface()?.DAdvise(format, advf, sink)?) }
    }

    /// Terminates an advisory connection.
    ///
    /// # Arguments
    /// * `connection` - Connection token from dadvise
    ///
    /// # Returns
    /// Ok(()) if connection was terminated successfully
    fn dunadvise(&self, connection: u32) -> OpcResult<()> {
        // SAFETY: Calling COM method DUnadvise.
        unsafe { Ok(self.interface()?.DUnadvise(connection)?) }
    }

    /// Enumerates active advisory connections.
    ///
    /// # Returns
    /// Enumerator for active advisory connections
    fn enum_dadvise(&self) -> OpcResult<windows::Win32::System::Com::IEnumSTATDATA> {
        // SAFETY: Calling COM method EnumDAdvise.
        unsafe { Ok(self.interface()?.EnumDAdvise()?) }
    }
}
