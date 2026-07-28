use crate::opc_da::errors::{OpcError, OpcResult};

/// Public group state management functionality.
///
/// Provides methods to manage public groups in OPC servers. Public groups
/// can be shared between multiple clients, allowing for more efficient
/// server resource usage.
pub trait PublicGroupStateMgtTrait {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCPublicGroupStateMgt>;

    /// Gets the public state of the group.
    ///
    /// # Returns
    /// `true` if the group is public, `false` if it is private
    fn get_state(&self) -> OpcResult<bool> {
        // SAFETY: Calling COM interface method GetState.
        unsafe { Ok(self.interface()?.GetState()?.as_bool()) }
    }

    /// Converts a private group to a public group.
    ///
    /// # Returns
    /// Ok(()) if the group was successfully converted to public
    ///
    /// # Notes
    /// Once a group becomes public, it remains public until the server
    /// is shut down or the group is deleted.
    fn move_to_public(&self) -> OpcResult<()> {
        // SAFETY: Calling COM interface method MoveToPublic.
        unsafe { Ok(self.interface()?.MoveToPublic()?) }
    }
}
