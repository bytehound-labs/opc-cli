use crate::opc_da::errors::{OpcError, OpcResult};
use windows::core::{GUID, Interface as _};

/// COM connection point container functionality.
///
/// Provides methods to establish connections between event sources
/// and event sinks in the OPC COM architecture. Used primarily for
/// handling asynchronous callbacks.
pub trait ConnectionPointContainerTrait {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer>;

    /// Finds a connection point for a specific interface.
    ///
    /// # Arguments
    /// * `id` - GUID of the connection point interface to find
    ///
    /// # Returns
    /// Connection point interface for the specified GUID
    ///
    /// # Safety  
    /// Caller must ensure:  
    /// - COM is properly initialized  
    /// - The underlying COM object is valid  
    ///
    /// # Errors  
    /// Returns an error if:  
    /// - The COM operation fails  
    /// - The connection point is not found
    fn find_connection_point(
        &self,
        id: &GUID,
    ) -> OpcResult<windows::Win32::System::Com::IConnectionPoint> {
        // SAFETY: Calling COM method FindConnectionPoint with valid IID reference.
        unsafe { Ok(self.interface()?.FindConnectionPoint(id)?) }
    }

    fn data_callback_connection_point(
        &self,
    ) -> OpcResult<windows::Win32::System::Com::IConnectionPoint> {
        self.find_connection_point(&crate::bindings::da::IOPCDataCallback::IID)
    }

    /// Enumerates all available connection points.
    ///
    /// # Returns
    /// Enumerator for iterating through available connection points
    ///
    /// # Safety  
    /// Caller must ensure:  
    /// - COM is properly initialized  
    /// - The underlying COM object is valid  
    ///
    /// # Errors  
    /// Returns an error if:  
    /// - The COM operation fails  
    /// - No connection points are available  
    fn enum_connection_points(
        &self,
    ) -> OpcResult<windows::Win32::System::Com::IEnumConnectionPoints> {
        // SAFETY: Calling COM method EnumConnectionPoints.
        unsafe { Ok(self.interface()?.EnumConnectionPoints()?) }
    }
}
