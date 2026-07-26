use windows::core::Interface as _;

use crate::opc_da::{
    client::{GroupIterator, StringIterator},
    com_utils::{LocalPointer, RemotePointer},
    errors::{OpcError, OpcResult},
    typedefs::GroupHandle,
};

/// OPC Server management functionality.
///
/// Provides methods to create and manage groups within an OPC server,
/// as well as monitor server status and enumerate existing groups.
pub trait ServerTrait<Group: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServer>;

    /// Adds a new group to the OPC server.
    ///
    /// # Arguments
    /// * `name` - Group name for identification
    /// * `active` - Whether the group should initially be active
    /// * `client_handle` - Client-assigned handle for the group
    /// * `update_rate` - Requested update rate in milliseconds
    /// * `locale_id` - Locale ID for text strings
    /// * `time_bias` - Time zone bias in minutes from UTC
    /// * `percent_deadband` - Percent change required to trigger updates
    ///
    /// # Returns
    /// The newly created group object
    ///
    /// # Errors
    /// Returns E_POINTER if group creation fails
    #[allow(clippy::too_many_arguments)]
    fn add_group(
        &self,
        name: &str,
        active: bool,
        update_rate: u32,
        client_handle: GroupHandle,
        time_bias: i32,
        percent_deadband: f32,
        locale_id: u32,
        revised_update_rate: &mut u32,
        server_handle: &mut GroupHandle,
    ) -> OpcResult<Group> {
        let mut group = None;
        let group_name = LocalPointer::from(name);
        let group_name = group_name.as_pcwstr();

        let mut raw_server_handle = 0u32;
        // SAFETY: Calling COM interface method AddGroup with group params and pointers.
        unsafe {
            self.interface()?.AddGroup(
                group_name,
                active,
                update_rate,
                client_handle.0,
                &time_bias,
                &percent_deadband,
                locale_id,
                &mut raw_server_handle,
                revised_update_rate,
                &crate::bindings::da::IOPCItemMgt::IID,
                &mut group,
            )?;
        }
        *server_handle = GroupHandle(raw_server_handle);

        match group {
            None => Err(OpcError::Com {
                source: windows::core::Error::new(
                    windows::Win32::Foundation::E_POINTER,
                    "Failed to add group, returned null",
                ),
            }),
            Some(group) => group
                .cast::<windows::core::IUnknown>()?
                .try_into()
                .map_err(|source| OpcError::Com { source }),
        }
    }

    /// Gets the current server status.
    ///
    /// # Returns
    /// Server status structure containing vendor info, time, state,
    /// and group counts
    fn get_status(&self) -> OpcResult<RemotePointer<crate::bindings::da::tagOPCSERVERSTATUS>> {
        // SAFETY: Calling COM interface method GetStatus.
        let status = unsafe { self.interface()?.GetStatus()? };
        Ok(RemotePointer::from_raw(status))
    }

    /// Removes a group from the server.
    ///
    /// # Arguments
    /// * `server_handle` - Server's handle for the group
    /// * `force` - If true, remove even if clients are connected
    fn remove_group(&self, server_handle: GroupHandle, force: bool) -> OpcResult<()> {
        // SAFETY: Calling COM interface method RemoveGroup with server_handle.
        unsafe {
            self.interface()?.RemoveGroup(server_handle.0, force)?;
        }
        Ok(())
    }

    /// Creates an enumerator for groups.
    ///
    /// # Arguments
    /// * `scope` - Scope of groups to enumerate (public, private, or all)
    ///
    /// # Returns
    /// Enumerator interface for iterating through groups
    fn create_group_enumerator(
        &self,
        scope: crate::bindings::da::tagOPCENUMSCOPE,
    ) -> OpcResult<GroupIterator<Group>> {
        // SAFETY: Calling COM interface method CreateGroupEnumerator with IEnumUnknown IID.
        let enumerator = unsafe {
            self.interface()?
                .CreateGroupEnumerator(scope, &windows::Win32::System::Com::IEnumUnknown::IID)?
        };

        Ok(GroupIterator::new(enumerator.cast()?))
    }

    /// Creates an enumerator for group names.
    ///
    /// # Arguments
    /// * `scope` - Scope of group names to enumerate (public, private, or all)
    ///
    /// # Returns
    /// Enumerator interface for iterating through group names
    fn create_group_name_enumerator(
        &self,
        scope: crate::bindings::da::tagOPCENUMSCOPE,
    ) -> OpcResult<StringIterator> {
        // SAFETY: Calling COM interface method CreateGroupEnumerator with IEnumString IID.
        let enumerator = unsafe {
            self.interface()?
                .CreateGroupEnumerator(scope, &windows::Win32::System::Com::IEnumString::IID)?
        };

        Ok(StringIterator::new(enumerator.cast()?))
    }
}
