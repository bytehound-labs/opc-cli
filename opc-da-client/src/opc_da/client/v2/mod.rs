//! OPC DA 2.0 server and group implementations.
//!
//! This module implements the OPC DA 2.0 interfaces for servers and groups,
//! providing compatibility with the 2.0 version of the specification.

use windows::core::Interface as _;

use crate::opc_da::errors::{OpcError, OpcResult};

use super::{
    ClientTrait,
    traits::{
        AsyncIo2Trait, AsyncIoTrait, BrowseServerAddressSpaceTrait, CommonTrait,
        ConnectionPointContainerTrait, DataObjectTrait, GroupStateMgtTrait, ItemMgtTrait,
        ItemPropertiesTrait, PublicGroupStateMgtTrait, ServerPublicGroupsTrait, ServerTrait,
        SyncIoTrait,
    },
};

/// Client for OPC DA 2.0 servers.
#[derive(Debug)]
pub struct Client;

impl ClientTrait<Server> for Client {
    const CATALOG_ID: windows::core::GUID = crate::bindings::da::CATID_OPCDAServer20::IID;
}

/// An OPC DA 2.0 server implementation.
///
/// Provides access to OPC DA 2.0 server interfaces including:
/// - `IOPCServer` for basic server operations
/// - `IOPCCommon` for server status and locale management
/// - optional `IOPCItemProperties` for browsing item properties
/// - `IOPCServerPublicGroups` for public group management
/// - `IOPCBrowseServerAddressSpace` for browsing the address space
pub struct Server {
    pub(crate) server: crate::bindings::da::IOPCServer,
    pub(crate) common: crate::bindings::comn::IOPCCommon,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) item_properties: Option<crate::bindings::da::IOPCItemProperties>,
    pub(crate) server_public_groups: Option<crate::bindings::da::IOPCServerPublicGroups>,
    pub(crate) browse_server_address_space:
        Option<crate::bindings::da::IOPCBrowseServerAddressSpace>,
}

impl TryFrom<windows::core::IUnknown> for Server {
    type Error = windows::core::Error;

    fn try_from(value: windows::core::IUnknown) -> windows::core::Result<Self> {
        Ok(Self {
            server: value.cast()?,
            common: value.cast()?,
            connection_point_container: value.cast()?,
            item_properties: value.cast().ok(),
            server_public_groups: value.cast().ok(),
            browse_server_address_space: value.cast().ok(),
        })
    }
}

impl ServerTrait<Group> for Server {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServer> {
        Ok(&self.server)
    }
}

impl CommonTrait for Server {
    fn interface(&self) -> OpcResult<&crate::bindings::comn::IOPCCommon> {
        Ok(&self.common)
    }
}

impl ConnectionPointContainerTrait for Server {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl ItemPropertiesTrait for Server {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemProperties> {
        self.item_properties
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCItemProperties not supported".to_string()))
    }
}

impl ServerPublicGroupsTrait for Server {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServerPublicGroups> {
        self.server_public_groups.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCServerPublicGroups not supported".to_string())
        })
    }
}

impl BrowseServerAddressSpaceTrait for Server {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCBrowseServerAddressSpace> {
        self.browse_server_address_space.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCBrowseServerAddressSpace not supported".to_string())
        })
    }
}

/// Iterator over OPC DA 2.0 groups.
pub type GroupIterator = super::GroupIterator<Group>;

/// An OPC DA 2.0 group implementation.
///
/// Provides access to OPC DA 2.0 group interfaces including:
/// - `IOPCItemMgt` for item management
/// - `IOPCGroupStateMgt` for group state management
/// - `IOPCPublicGroupStateMgt` for public group operations
/// - `IOPCSyncIO` for synchronous operations
/// - `IOPCAsyncIO` and `IOPCAsyncIO2` for asynchronous operations
/// - `IDataObject` for data transfer
pub struct Group {
    pub(crate) item_mgt: crate::bindings::da::IOPCItemMgt,
    pub(crate) group_state_mgt: crate::bindings::da::IOPCGroupStateMgt,
    pub(crate) public_group_state_mgt: Option<crate::bindings::da::IOPCPublicGroupStateMgt>,
    pub(crate) sync_io: crate::bindings::da::IOPCSyncIO,
    pub(crate) async_io: Option<crate::bindings::da::IOPCAsyncIO>,
    pub(crate) async_io2: crate::bindings::da::IOPCAsyncIO2,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) data_object: Option<windows::Win32::System::Com::IDataObject>,
}

impl TryFrom<windows::core::IUnknown> for Group {
    type Error = windows::core::Error;

    fn try_from(value: windows::core::IUnknown) -> windows::core::Result<Self> {
        Ok(Self {
            item_mgt: value.cast()?,
            group_state_mgt: value.cast()?,
            public_group_state_mgt: value.cast().ok(),
            sync_io: value.cast()?,
            async_io: value.cast().ok(),
            async_io2: value.cast()?,
            connection_point_container: value.cast()?,
            data_object: value.cast().ok(),
        })
    }
}

impl ItemMgtTrait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemMgt> {
        Ok(&self.item_mgt)
    }
}

impl GroupStateMgtTrait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCGroupStateMgt> {
        Ok(&self.group_state_mgt)
    }
}

impl PublicGroupStateMgtTrait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCPublicGroupStateMgt> {
        self.public_group_state_mgt.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCPublicGroupStateMgt not supported".to_string())
        })
    }
}

impl SyncIoTrait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO> {
        Ok(&self.sync_io)
    }
}

impl AsyncIoTrait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO> {
        self.async_io
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCAsyncIO not supported".to_string()))
    }
}

impl AsyncIo2Trait for Group {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO2> {
        Ok(&self.async_io2)
    }
}

impl ConnectionPointContainerTrait for Group {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl DataObjectTrait for Group {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IDataObject> {
        self.data_object
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IDataObject not supported".to_string()))
    }
}
