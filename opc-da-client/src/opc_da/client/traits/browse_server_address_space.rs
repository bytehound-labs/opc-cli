use crate::bindings::da::{
    IOPCBrowseServerAddressSpace, tagOPCBROWSEDIRECTION, tagOPCBROWSETYPE, tagOPCNAMESPACETYPE,
};
use crate::opc_da::{
    client::StringIterator,
    com_utils::{LocalPointer, RemotePointer},
    errors::{OpcError, OpcResult},
};

/// Server address space browsing functionality.
///
/// Provides methods to navigate and discover items in the OPC server's address space.
/// Supports hierarchical and flat address spaces with filtering capabilities.
pub trait BrowseServerAddressSpaceTrait {
    fn interface(&self) -> OpcResult<&IOPCBrowseServerAddressSpace>;

    /// Queries the organization type of the server's address space.
    ///
    /// # Returns
    /// The namespace type (hierarchical or flat)
    fn query_organization(&self) -> OpcResult<tagOPCNAMESPACETYPE> {
        // SAFETY: Calling COM interface method QueryOrganization.
        unsafe { Ok(self.interface()?.QueryOrganization()?) }
    }

    /// Changes the current position in the server's address space.
    ///
    /// # Arguments
    /// * `browse_direction` - Direction to move (up, down, or to)
    /// * `position` - Target position string (branch name for down/to)
    ///
    /// # Returns
    /// Result indicating success or failure of position change
    fn change_browse_position(
        &self,
        browse_direction: tagOPCBROWSEDIRECTION,
        position: &str,
    ) -> OpcResult<()> {
        let position = LocalPointer::from(position);

        // SAFETY: Calling COM interface method ChangeBrowsePosition with valid position string.
        unsafe {
            Ok(self
                .interface()?
                .ChangeBrowsePosition(browse_direction, position.as_pwstr())?)
        }
    }

    /// Browses available item IDs at the current position.
    ///
    /// # Arguments
    /// * `browse_type` - Type of items to browse (leaf, branch, or flat)
    /// * `filter_criteria` - Pattern for filtering items
    /// * `datatype_filter` - VT_* type to filter by (0 for all)
    /// * `access_rights_filter` - Required access rights
    ///
    /// # Returns
    /// Enumerator for matching item IDs
    fn browse_opc_item_ids<S0>(
        &self,
        browse_type: tagOPCBROWSETYPE,
        filter_criteria: Option<S0>,
        data_type_filter: u16,
        access_rights_filter: u32,
    ) -> OpcResult<StringIterator>
    where
        S0: AsRef<str>,
    {
        let filter_criteria = LocalPointer::from_option(filter_criteria);

        // SAFETY: Calling COM interface method BrowseOPCItemIDs with valid filter string.
        unsafe {
            let iter = self.interface()?.BrowseOPCItemIDs(
                browse_type,
                filter_criteria.as_pwstr(),
                data_type_filter,
                access_rights_filter,
            )?;
            Ok(StringIterator::new(iter))
        }
    }

    /// Gets fully qualified item ID from a leaf item.
    ///
    /// # Arguments
    /// * `item_data_id` - Item name at current position
    ///
    /// # Returns
    /// Fully qualified item ID string
    fn get_item_id(&self, item_data_id: &str) -> OpcResult<String> {
        let item_data_id = LocalPointer::from(item_data_id);

        // SAFETY: Calling COM interface method GetItemID with valid item_data_id string.
        let output = unsafe { self.interface()?.GetItemID(item_data_id.as_pwstr())? };

        let ptr = RemotePointer::from(output);
        ptr.try_into().map_err(OpcError::from)
    }

    /// Browses available access paths for an item.
    ///
    /// # Arguments
    /// * `item_id` - Fully qualified item ID
    ///
    /// # Returns
    /// Enumerator for available access paths
    fn browse_access_paths(&self, item_id: &str) -> OpcResult<StringIterator> {
        let item_id = LocalPointer::from(item_id);
        // SAFETY: Calling COM interface method BrowseAccessPaths with valid item_id string.
        unsafe {
            let iter = self.interface()?.BrowseAccessPaths(item_id.as_pwstr())?;
            Ok(StringIterator::new(iter))
        }
    }
}
