//! Abstractions for OPC DA server connectivity.
//!
//! Defines the [`ServerConnector`], [`ConnectedServer`], and [`ConnectedGroup`]
//! traits that decouple [`super::opc_da::OpcDaClient`] from concrete COM types.
//! This enables mock implementations for unit testing without a live COM server.

pub use crate::bindings::da::tagOPCITEMDEF;
pub use crate::bindings::da::{tagOPCITEMRESULT, tagOPCITEMSTATE};
pub use crate::opc_da::client::*;
pub use crate::opc_da::com_utils::RemoteArray;
pub use crate::opc_da::errors::{OpcError, OpcResult};
use crate::provider::BrowseNodeFilter;
use anyhow::Context;
pub use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface;

/// Rust-native OPC DA 3.0 browse element used inside the backend boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBrowseElement {
    pub(crate) name: String,
    pub(crate) item_id: Option<String>,
    pub(crate) has_children: bool,
    pub(crate) is_item: bool,
}

/// Rust-native OPC DA 3.0 page used inside the backend boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBrowsePage {
    pub(crate) elements: Vec<NativeBrowseElement>,
    pub(crate) more_elements: bool,
    pub(crate) continuation: Option<String>,
}

/// Object-safe string enumerator used to keep DA 2.x COM enumeration state on
/// the worker while allowing tests to supply pure Rust iterators.
pub trait BrowseStringIterator {
    fn next_string(&mut self) -> Option<OpcResult<String>>;
}

impl<T> BrowseStringIterator for T
where
    T: Iterator<Item = OpcResult<String>>,
{
    fn next_string(&mut self) -> Option<OpcResult<String>> {
        self.next()
    }
}

/// Factory for connecting to OPC DA servers.
///
/// Abstracts the concrete COM client usage so that tests can inject mocks
/// that return pre-configured server/group results without a live COM runtime.
///
/// # Errors
///
/// All methods return `OpcResult` — implementations should wrap COM errors
/// with contextual messages.
pub trait ServerConnector: Send + Sync {
    /// The server facade type returned by [`Self::connect`].
    type Server: ConnectedServer;

    /// Enumerate all OPC DA server ProgIDs on the local machine.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM registry enumeration fails.
    fn enumerate_servers(&self) -> OpcResult<Vec<String>>;

    /// Connect to the named OPC DA server and return a server facade.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM server cannot be created or connected.
    fn connect(&self, server_name: &str) -> OpcResult<Self::Server>;
}

/// Facade over a connected OPC DA server instance.
///
/// Wraps namespace browsing and group management operations in Rust-native types.
///
/// # Errors
///
/// All methods return `OpcResult` — COM errors are propagated with context.
pub trait ConnectedServer {
    /// The group facade type returned by [`Self::add_group`].
    type Group: ConnectedGroup;

    /// Query the server's namespace organization type.
    ///
    /// Returns `OPC_NS_FLAT` or `OPC_NS_HIERARCHICAL` as a `u32`.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM call fails.
    fn query_organization(&self) -> OpcResult<u32>;

    /// Browse the server's address space for item IDs of the given type.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM browse call fails.
    fn browse_opc_item_ids(
        &self,
        browse_type: u32,
        filter: Option<&str>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<StringIterator>;

    /// Change the current browse position (e.g., navigate into/out of branches).
    ///
    /// # Errors
    ///
    /// Returns an error if the position change is rejected by the server.
    fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()>;

    /// Resolve a browse name to its fully-qualified item ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot resolve the item name.
    fn get_item_id(&self, item_name: &str) -> OpcResult<String>;

    /// Resolve a DA 2.x browse name only when it is also an item.
    ///
    /// OPC DA servers commonly report `OPC_E_UNKNOWNITEMID` or
    /// `OPC_E_INVALIDITEMID` when `GetItemID` is called for a branch-only
    /// browse name. Other failures remain hard errors.
    fn resolve_da2_item_id(&self, item_name: &str) -> OpcResult<Option<String>> {
        match self.get_item_id(item_name) {
            Ok(item_id) => Ok(Some(item_id)),
            Err(OpcError::Com { source })
                if matches!(source.code().0.cast_unsigned(), 0xC004_0007 | 0xC004_0008) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Return whether a DA 2.x item name also identifies a child branch.
    ///
    /// Backends that can probe branch navigation should override this method.
    fn da2_name_has_children(&self, _item_name: &str) -> OpcResult<bool> {
        Ok(false)
    }

    /// Return whether OPC DA 2.x address-space browsing is available.
    fn supports_da2_browse(&self) -> bool {
        true
    }

    /// Return whether OPC DA 3.0 native browsing is available.
    fn supports_da3_browse(&self) -> bool {
        false
    }

    /// Start a stateful OPC DA 2.x string enumeration.
    ///
    /// The returned iterator remains on the COM worker and is never exposed
    /// through the public API.
    fn begin_da2_browse(
        &self,
        browse_type: u32,
        filter: Option<&str>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<Box<dyn BrowseStringIterator>> {
        Ok(Box::new(self.browse_opc_item_ids(
            browse_type,
            filter,
            data_type,
            access_rights,
        )?))
    }

    /// Return one native OPC DA 3.0 browse page.
    ///
    /// The continuation value is backend-private and is replaced with an
    /// opaque random token before crossing the public API boundary.
    fn browse_da3(
        &self,
        _item_id: Option<&str>,
        _continuation: Option<&str>,
        _max_elements: u32,
        _filter: BrowseNodeFilter,
    ) -> OpcResult<NativeBrowsePage> {
        Err(OpcError::NotImplemented(
            "IOPCBrowse is not supported".to_string(),
        ))
    }

    /// Add a new OPC group to this server connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the group creation fails.
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
    ) -> OpcResult<Self::Group>;

    /// Remove an OPC group by its server-assigned handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the group removal fails.
    fn remove_group(&self, server_group: GroupHandle, force: bool) -> OpcResult<()>;
}

/// Facade over an OPC DA group for item management and I/O.
///
/// # Errors
///
/// All methods return `OpcResult` — COM errors are propagated with context.
pub trait ConnectedGroup {
    /// Add items to this group for monitoring.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `AddItems` call fails.
    fn add_items(
        &self,
        items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )>;

    /// Perform a synchronous read of the given server handles.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `Read` call fails.
    fn read(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMSTATE>,
        RemoteArray<windows::core::HRESULT>,
    )>;

    /// Write values to the given server handles.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `Write` call fails.
    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>>;
}

// ── COM-backed implementations ──────────────────────────────────────

/// Real COM-backed server connector implementation.
///
/// Uses Windows COM to enumerate and connect to OPC DA servers.
pub struct ComConnector;

impl ServerConnector for ComConnector {
    type Server = ComServer;

    fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
        let client = crate::opc_da::client::v2::Client;
        let guid_iter = client
            .get_servers()
            .context("Failed to enumerate OPC DA servers from registry")?;

        let mut servers = Vec::new();
        for guid in guid_iter.flatten() {
            // SAFETY: `crate::opc_da::GUID` and `windows::core::GUID` are both `#[repr(C)]` structs with identical layout.
            // SAFETY: Validated by a `const_assert_eq!` in `opc_da/client/iterator.rs`.
            let win_guid: windows::core::GUID = unsafe { std::mem::transmute_copy(&guid) };
            if win_guid == windows::core::GUID::zeroed() {
                continue;
            }

            if let Ok(progid) = crate::helpers::guid_to_progid(&win_guid)
                && !progid.is_empty()
            {
                servers.push(progid);
            }
        }
        servers.sort();
        servers.dedup();
        Ok(servers)
    }

    fn connect(&self, server_name: &str) -> OpcResult<Self::Server> {
        let opc_server = crate::helpers::connect_server(server_name)?;
        let unknown: windows::core::IUnknown = opc_server.cast()?;

        Ok(ComServer {
            server: opc_server,
            common: unknown.cast()?,
            connection_point_container: unknown.cast()?,
            item_properties: unknown.cast()?,
            server_public_groups: unknown.cast().ok(),
            browse_server_address_space: unknown.cast().ok(),
            browse: unknown.cast().ok(),
        })
    }
}

/// COM-backed [`ConnectedServer`].
pub struct ComServer {
    pub(crate) server: crate::bindings::da::IOPCServer,
    pub(crate) common: crate::bindings::comn::IOPCCommon,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) item_properties: crate::bindings::da::IOPCItemProperties,
    pub(crate) server_public_groups: Option<crate::bindings::da::IOPCServerPublicGroups>,
    pub(crate) browse_server_address_space:
        Option<crate::bindings::da::IOPCBrowseServerAddressSpace>,
    pub(crate) browse: Option<crate::bindings::da::IOPCBrowse>,
}

impl ServerTrait<ComGroup> for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServer> {
        Ok(&self.server)
    }
}

impl CommonTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::comn::IOPCCommon> {
        Ok(&self.common)
    }
}

impl ConnectionPointContainerTrait for ComServer {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl ItemPropertiesTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemProperties> {
        Ok(&self.item_properties)
    }
}

impl ServerPublicGroupsTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServerPublicGroups> {
        self.server_public_groups.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCServerPublicGroups not supported".to_string())
        })
    }
}

impl BrowseServerAddressSpaceTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCBrowseServerAddressSpace> {
        self.browse_server_address_space.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCBrowseServerAddressSpace not supported".to_string())
        })
    }
}

impl BrowseTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCBrowse> {
        self.browse
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCBrowse not supported".to_string()))
    }
}

impl ConnectedServer for ComServer {
    type Group = ComGroup;

    fn query_organization(&self) -> OpcResult<u32> {
        let org = BrowseServerAddressSpaceTrait::query_organization(self)?;
        Ok(org.0.cast_unsigned())
    }

    fn browse_opc_item_ids(
        &self,
        browse_type: u32,
        filter: Option<&str>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<StringIterator> {
        BrowseServerAddressSpaceTrait::browse_opc_item_ids(
            self,
            crate::bindings::da::tagOPCBROWSETYPE(browse_type.cast_signed()),
            filter,
            data_type,
            access_rights,
        )
    }

    fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
        BrowseServerAddressSpaceTrait::change_browse_position(
            self,
            crate::bindings::da::tagOPCBROWSEDIRECTION(direction.cast_signed()),
            name,
        )
    }

    fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
        BrowseServerAddressSpaceTrait::get_item_id(self, item_name)
    }

    fn da2_name_has_children(&self, item_name: &str) -> OpcResult<bool> {
        let down = crate::bindings::da::OPC_BROWSE_DOWN.0.cast_unsigned();
        let up = crate::bindings::da::OPC_BROWSE_UP.0.cast_unsigned();
        match ConnectedServer::change_browse_position(self, down, item_name) {
            Ok(()) => {
                ConnectedServer::change_browse_position(self, up, "")?;
                Ok(true)
            }
            Err(OpcError::Com { source })
                if !matches!(
                    source.code().0.cast_unsigned(),
                    0x8007_06BA | 0x8007_06BF | 0x8007_06BE | 0x8008_0005
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn supports_da2_browse(&self) -> bool {
        self.browse_server_address_space.is_some()
    }

    fn supports_da3_browse(&self) -> bool {
        self.browse.is_some()
    }

    fn browse_da3(
        &self,
        item_id: Option<&str>,
        continuation: Option<&str>,
        max_elements: u32,
        filter: BrowseNodeFilter,
    ) -> OpcResult<NativeBrowsePage> {
        use crate::bindings::da::{
            OPC_BROWSE_FILTER_ALL, OPC_BROWSE_FILTER_BRANCHES, OPC_BROWSE_FILTER_ITEMS,
            OPC_BROWSE_HASCHILDREN, OPC_BROWSE_ISITEM,
        };
        use crate::opc_da::com_utils::RemotePointer;

        let native_filter = match filter {
            BrowseNodeFilter::Branches => OPC_BROWSE_FILTER_BRANCHES,
            BrowseNodeFilter::Items => OPC_BROWSE_FILTER_ITEMS,
            BrowseNodeFilter::All => OPC_BROWSE_FILTER_ALL,
        };
        let (more_elements, continuation, elements) = BrowseTrait::browse(
            self,
            item_id,
            continuation,
            max_elements,
            native_filter,
            None::<&str>,
            None::<&str>,
            false,
            false,
            &[],
        )?;

        let owned_strings: Vec<_> = elements
            .as_slice()
            .iter()
            .map(|element| {
                (
                    RemotePointer::from(element.szName),
                    RemotePointer::from(element.szItemID),
                    element.dwFlagValue,
                )
            })
            .collect();

        let mut mapped = Vec::with_capacity(owned_strings.len());
        for (name, item_id, flags) in owned_strings {
            mapped.push(NativeBrowseElement {
                name: String::try_from(name)?,
                item_id: Option::<String>::try_from(item_id)?.filter(|value| !value.is_empty()),
                has_children: flags & OPC_BROWSE_HASCHILDREN != 0,
                is_item: flags & OPC_BROWSE_ISITEM != 0,
            });
        }

        Ok(NativeBrowsePage {
            elements: mapped,
            more_elements,
            continuation: continuation.filter(|value| !value.is_empty()),
        })
    }

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
    ) -> OpcResult<Self::Group> {
        ServerTrait::add_group(
            self,
            name,
            active,
            update_rate,
            client_handle,
            time_bias,
            percent_deadband,
            locale_id,
            revised_update_rate,
            server_handle,
        )
    }

    fn remove_group(&self, server_group: GroupHandle, force: bool) -> OpcResult<()> {
        ServerTrait::remove_group(self, server_group, force)
    }
}

pub struct ComGroup {
    pub(crate) item_mgt: crate::bindings::da::IOPCItemMgt,
    pub(crate) group_state_mgt: crate::bindings::da::IOPCGroupStateMgt,
    pub(crate) public_group_state_mgt: Option<crate::bindings::da::IOPCPublicGroupStateMgt>,
    pub(crate) sync_io: crate::bindings::da::IOPCSyncIO,
    pub(crate) async_io: Option<crate::bindings::da::IOPCAsyncIO>,
    pub(crate) async_io2: crate::bindings::da::IOPCAsyncIO2,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) data_object: Option<windows::Win32::System::Com::IDataObject>,
}

impl ItemMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemMgt> {
        Ok(&self.item_mgt)
    }
}

impl GroupStateMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCGroupStateMgt> {
        Ok(&self.group_state_mgt)
    }
}

impl PublicGroupStateMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCPublicGroupStateMgt> {
        self.public_group_state_mgt.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCPublicGroupStateMgt not supported".to_string())
        })
    }
}

impl SyncIoTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO> {
        Ok(&self.sync_io)
    }
}

impl AsyncIoTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO> {
        self.async_io
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCAsyncIO not supported".to_string()))
    }
}

impl AsyncIo2Trait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO2> {
        Ok(&self.async_io2)
    }
}

impl ConnectionPointContainerTrait for ComGroup {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl DataObjectTrait for ComGroup {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IDataObject> {
        self.data_object
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IDataObject not supported".to_string()))
    }
}

impl ConnectedGroup for ComGroup {
    fn add_items(
        &self,
        items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        ItemMgtTrait::add_items(self, items)
    }

    fn read(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMSTATE>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        SyncIoTrait::read(self, source, server_handles)
    }

    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        SyncIoTrait::write(self, server_handles, values)
    }
}

impl TryFrom<windows::core::IUnknown> for ComGroup {
    type Error = windows::core::Error;

    fn try_from(unknown: windows::core::IUnknown) -> Result<Self, Self::Error> {
        Ok(Self {
            item_mgt: unknown.cast()?,
            group_state_mgt: unknown.cast()?,
            public_group_state_mgt: unknown.cast().ok(),
            sync_io: unknown.cast()?,
            async_io: unknown.cast().ok(),
            async_io2: unknown.cast()?,
            connection_point_container: unknown.cast()?,
            data_object: unknown.cast().ok(),
        })
    }
}
