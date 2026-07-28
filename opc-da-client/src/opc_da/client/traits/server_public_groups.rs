use crate::opc_da::{com_utils::LocalPointer, errors::OpcResult};

/// Server public groups management functionality.
///
/// Provides methods to access and manage public groups that can be shared
/// between multiple OPC clients for more efficient server resource usage.
pub trait ServerPublicGroupsTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServerPublicGroups>;

    /// Gets a public group by its name.
    ///
    /// # Arguments
    /// * `name` - Name of the public group to retrieve
    /// * `id` - Interface ID for the requested group interface
    ///
    /// # Returns
    /// The requested interface pointer for the public group
    fn get_public_group_by_name(
        &self,
        name: &str,
        id: &windows::core::GUID,
    ) -> OpcResult<windows::core::IUnknown> {
        let name = LocalPointer::from(name);

        // SAFETY: Calling COM interface method GetPublicGroupByName with valid name string pointer.
        unsafe {
            Ok(self
                .interface()?
                .GetPublicGroupByName(name.as_pcwstr(), id)?)
        }
    }

    /// Removes a public group from the server.
    ///
    /// # Arguments
    /// * `server_group` - Server handle of the group to remove
    /// * `force` - If true, removes group even if clients are connected
    ///
    /// # Returns
    /// Ok(()) if the group was successfully removed
    fn remove_public_group(&self, server_group: u32, force: bool) -> OpcResult<()> {
        // SAFETY: Calling COM interface method RemovePublicGroup with server_group handle.
        unsafe { Ok(self.interface()?.RemovePublicGroup(server_group, force)?) }
    }
}
