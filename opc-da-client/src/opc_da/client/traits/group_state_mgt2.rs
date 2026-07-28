use crate::opc_da::errors::OpcResult;

/// Extended group state management trait (OPC DA 3.0).
///
/// Provides methods to manage the keep-alive time for OPC groups. The keep-alive time
/// determines how often the server sends keep-alive notifications to maintain the
/// connection state, even when no data has changed.
pub trait GroupStateMgt2Trait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCGroupStateMgt2>;

    /// Sets the keep-alive time for the group in milliseconds.
    ///
    /// # Arguments
    /// * `keep_alive_time` - The requested keep-alive time in milliseconds
    ///
    /// # Returns
    /// The actual keep-alive time set by the server, which may differ from
    /// the requested time based on server capabilities
    ///
    /// # Notes
    /// The server may not support the exact requested time and will return
    /// the closest supported value. A value of 0 typically disables keep-alive.
    fn set_keep_alive(&self, keep_alive_time: u32) -> OpcResult<u32> {
        // SAFETY: Calling COM interface method SetKeepAlive.
        unsafe { Ok(self.interface()?.SetKeepAlive(keep_alive_time)?) }
    }

    /// Gets the current keep-alive time for the group.
    ///
    /// # Returns
    /// The current keep-alive time in milliseconds. A value of 0 indicates
    /// that keep-alive is disabled.
    fn get_keep_alive(&self) -> OpcResult<u32> {
        // SAFETY: Calling COM interface method GetKeepAlive.
        unsafe { Ok(self.interface()?.GetKeepAlive()?) }
    }
}
