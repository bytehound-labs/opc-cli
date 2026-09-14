//! Abstractions for OPC DA server connectivity.
//!
//! Defines the [`ServerConnector`], [`ConnectedServer`], and [`ConnectedGroup`]
//! traits that decouple [`super::opc_da::OpcDaClient`] from concrete COM types.
//! This enables mock implementations for unit testing without a live COM server.

pub use crate::bindings::da::tagOPCITEMDEF;
pub use crate::bindings::da::{tagOPCITEMRESULT, tagOPCITEMSTATE};
pub use crate::opc_da::client::*;
pub use crate::opc_da::com_utils::RemoteArray;
use crate::opc_da::errors::{E_INVALIDARG_HRESULT, is_com_hresult};
pub use crate::opc_da::errors::{OpcError, OpcResult};
use crate::provider::BrowseNodeFilter;
use anyhow::Context;
pub use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface;

#[cfg(feature = "dev-diagnostics")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticServerStatus {
    pub(crate) start_time: std::time::SystemTime,
    pub(crate) current_time: std::time::SystemTime,
    pub(crate) last_update_time: std::time::SystemTime,
    pub(crate) server_state: String,
    pub(crate) group_count: u32,
    pub(crate) band_width: u32,
    pub(crate) major_version: u16,
    pub(crate) minor_version: u16,
    pub(crate) build_number: u16,
    pub(crate) vendor_info: String,
}

#[cfg(feature = "dev-diagnostics")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticItemProperty {
    pub(crate) id: u32,
    pub(crate) description: String,
    pub(crate) data_type: u16,
    pub(crate) value: Option<String>,
    pub(crate) error: windows::core::HRESULT,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Da2BranchNavigation {
    Navigable,
    RejectedInvalidArgument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Da2BranchClassification {
    pub item_id: Option<String>,
    pub navigation: Da2BranchNavigation,
}

pub fn classify_da2_branch<S: ConnectedServer>(
    server: &S,
    item_name: &str,
) -> OpcResult<Da2BranchClassification> {
    let item_id = match server.resolve_da2_item_id(item_name) {
        Ok(item_id) => item_id,
        Err(error) if is_com_hresult(&error, E_INVALIDARG_HRESULT) => None,
        Err(error) => return Err(error),
    };
    let down = crate::bindings::da::OPC_BROWSE_DOWN.0.cast_unsigned();
    let up = crate::bindings::da::OPC_BROWSE_UP.0.cast_unsigned();
    let navigation = match server.change_browse_position(down, item_name) {
        Ok(()) => {
            server.change_browse_position(up, "")?;
            Da2BranchNavigation::Navigable
        }
        Err(error) if is_com_hresult(&error, E_INVALIDARG_HRESULT) => {
            Da2BranchNavigation::RejectedInvalidArgument
        }
        Err(error) => return Err(error),
    };
    Ok(Da2BranchClassification {
        item_id,
        navigation,
    })
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

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_status(&self) -> OpcResult<DiagnosticServerStatus> {
        Err(OpcError::NotImplemented(
            "OPC server status diagnostics are not supported".to_string(),
        ))
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_locale_id(&self) -> OpcResult<u32> {
        Err(OpcError::NotImplemented(
            "OPC locale diagnostics are not supported".to_string(),
        ))
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_available_locale_ids(&self) -> OpcResult<Vec<u32>> {
        Err(OpcError::NotImplemented(
            "OPC locale enumeration diagnostics are not supported".to_string(),
        ))
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_error_string(&self, _error: windows::core::HRESULT) -> OpcResult<String> {
        Err(OpcError::NotImplemented(
            "OPC vendor error-string diagnostics are not supported".to_string(),
        ))
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_item_properties(&self, _item_id: &str) -> OpcResult<Vec<DiagnosticItemProperty>> {
        Err(OpcError::NotImplemented(
            "OPC item-property diagnostics are not supported".to_string(),
        ))
    }
}

/// Facade over an OPC DA group for item management and I/O.
///
/// # Errors
///
/// All methods return `OpcResult` — COM errors are propagated with context.
pub trait ConnectedGroup {
    #[cfg(feature = "dev-diagnostics")]
    fn validate_items(
        &self,
        _items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        Err(OpcError::NotImplemented(
            "OPC item validation diagnostics are not supported".to_string(),
        ))
    }

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
            item_properties: unknown.cast().ok(),
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
    pub(crate) item_properties: Option<crate::bindings::da::IOPCItemProperties>,
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
        self.item_properties
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCItemProperties not supported".to_string()))
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
        use crate::opc_da::com_utils::TryFromNative;

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

        let mut mapped = Vec::with_capacity(elements.as_slice().len());
        for element in elements.as_slice() {
            let name = String::try_from_native(&element.szName)?;
            let item_id = if element.szItemID.is_null() {
                None
            } else {
                Some(String::try_from_native(&element.szItemID)?)
            };
            mapped.push(NativeBrowseElement {
                name,
                item_id: item_id.filter(|value| !value.is_empty()),
                has_children: element.dwFlagValue & OPC_BROWSE_HASCHILDREN != 0,
                is_item: element.dwFlagValue & OPC_BROWSE_ISITEM != 0,
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

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_status(&self) -> OpcResult<DiagnosticServerStatus> {
        use crate::opc_da::com_utils::TryFromNative;
        use crate::opc_da::typedefs::{ServerState, ServerStatus};

        let status = ServerTrait::get_status(self)?;
        let native = status
            .as_ref()
            .ok_or_else(|| OpcError::Internal("OPC server returned null status".to_string()))?;
        let status = ServerStatus::try_from_native(native)?;
        let server_state = match status.server_state {
            ServerState::Running => "running",
            ServerState::Failed => "failed",
            ServerState::NoConfig => "no_config",
            ServerState::Suspended => "suspended",
            ServerState::Test => "test",
            ServerState::CommunicationFault => "communication_fault",
        }
        .to_string();
        Ok(DiagnosticServerStatus {
            start_time: status.start_time,
            current_time: status.current_time,
            last_update_time: status.last_update_time,
            server_state,
            group_count: status.group_count,
            band_width: status.band_width,
            major_version: status.major_version,
            minor_version: status.minor_version,
            build_number: status.build_number,
            vendor_info: status.vendor_info,
        })
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_locale_id(&self) -> OpcResult<u32> {
        CommonTrait::get_locale_id(self)
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_available_locale_ids(&self) -> OpcResult<Vec<u32>> {
        Ok(CommonTrait::query_available_locale_ids(self)?
            .as_slice()
            .to_vec())
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_error_string(&self, error: windows::core::HRESULT) -> OpcResult<String> {
        CommonTrait::get_error_string(self, error)
    }

    #[cfg(feature = "dev-diagnostics")]
    fn diagnostic_item_properties(&self, item_id: &str) -> OpcResult<Vec<DiagnosticItemProperty>> {
        use crate::opc_da::com_utils::TryFromNative;

        let (ids, descriptions, data_types) =
            ItemPropertiesTrait::query_available_properties(self, item_id)?;
        if ids.len() != descriptions.len() || ids.len() != data_types.len() {
            return Err(OpcError::Internal(
                "OPC server returned mismatched available-property array sizes".to_string(),
            ));
        }

        let descriptions = descriptions
            .as_slice()
            .iter()
            .map(|description| String::try_from_native(description).map_err(OpcError::from))
            .collect::<OpcResult<Vec<_>>>()?;
        let standard_indices = ids
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, id)| is_standard_property_id(**id))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let standard_ids = standard_indices
            .iter()
            .map(|index| ids.as_slice()[*index])
            .collect::<Vec<_>>();
        if standard_ids.is_empty() {
            return Ok(Vec::new());
        }

        let (values, errors) =
            ItemPropertiesTrait::get_item_properties(self, item_id, &standard_ids)?;
        if values.len() as usize != standard_ids.len()
            || errors.len() as usize != standard_ids.len()
        {
            return Err(OpcError::Internal(
                "OPC server returned mismatched item-property result array sizes".to_string(),
            ));
        }

        Ok(standard_indices
            .into_iter()
            .enumerate()
            .map(|(value_index, available_index)| {
                let error = errors.as_slice()[value_index];
                DiagnosticItemProperty {
                    id: ids.as_slice()[available_index],
                    description: descriptions[available_index].clone(),
                    data_type: data_types.as_slice()[available_index],
                    value: error.is_ok().then(|| {
                        crate::helpers::variant_to_string(&values.as_slice()[value_index])
                    }),
                    error,
                }
            })
            .collect())
    }
}

#[cfg(feature = "dev-diagnostics")]
fn is_standard_property_id(id: u32) -> bool {
    (1..=8).contains(&id) || (100..=108).contains(&id)
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
    #[cfg(feature = "dev-diagnostics")]
    fn validate_items(
        &self,
        items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        ItemMgtTrait::validate_items(self, items, false)
    }

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
