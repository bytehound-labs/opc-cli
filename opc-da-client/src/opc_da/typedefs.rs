use crate::opc_da::com_utils::{
    IntoBridge, LocalPointer, RemoteArray, ToNative, TryFromNative, TryToNative, clear_pwstr,
    clear_variant, clone_variant, task_mem_allocation_bytes,
};
use crate::try_from_native;
use windows::Win32::System::Com::CoTaskMemFree;

unsafe fn copy_com_slice<T: Copy>(
    pointer: *const T,
    len: u32,
    field: &str,
) -> windows::core::Result<Vec<T>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_POINTER,
            format!("{field} pointer is null for {len} elements"),
        ));
    }
    let len = usize::try_from(len).map_err(|_| {
        windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            format!("{field} length does not fit in usize"),
        )
    })?;
    let byte_len = core::mem::size_of::<T>().checked_mul(len).ok_or_else(|| {
        windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            format!("{field} byte length overflows usize"),
        )
    })?;
    if byte_len > isize::MAX as usize {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            format!("{field} byte length exceeds isize::MAX"),
        ));
    }
    let allocation_bytes =
        unsafe { task_mem_allocation_bytes(pointer.cast::<core::ffi::c_void>()) }.ok_or_else(
            || {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    format!("{field} is not a valid COM task allocation"),
                )
            },
        )?;
    if byte_len > allocation_bytes {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            format!(
                "{field} requires {byte_len} bytes, but its COM allocation is only \
                 {allocation_bytes} bytes"
            ),
        ));
    }
    // SAFETY: The caller supplies a COM-returned pointer and its element count.
    Ok(unsafe { core::slice::from_raw_parts(pointer, len) }.to_vec())
}

unsafe fn clear_com_blob(pointer: &mut *mut u8) {
    if !pointer.is_null() {
        // SAFETY: OPC DA allocates returned blobs with the COM task allocator.
        unsafe { CoTaskMemFree(Some(pointer.cast())) };
        *pointer = core::ptr::null_mut();
    }
}

pub(crate) unsafe fn clear_server_status(native: *mut crate::bindings::da::tagOPCSERVERSTATUS) {
    // SAFETY: The caller provides a valid COM-returned server-status pointer.
    unsafe { clear_pwstr(&mut (*native).szVendorInfo) };
}

pub(crate) unsafe fn clear_item_result(native: &mut crate::bindings::da::tagOPCITEMRESULT) {
    // SAFETY: The blob is nested in this COM-returned item result.
    unsafe { clear_com_blob(&mut native.pBlob) };
    native.dwBlobSize = 0;
}

pub(crate) unsafe fn clear_item_state(native: &mut crate::bindings::da::tagOPCITEMSTATE) {
    // SAFETY: The VARIANT is nested in this COM-returned item state.
    unsafe { clear_variant(&mut native.vDataValue) };
}

pub(crate) unsafe fn clear_item_attributes(native: &mut crate::bindings::da::tagOPCITEMATTRIBUTES) {
    // SAFETY: These allocations are nested in this COM-returned item-attributes value.
    unsafe {
        clear_pwstr(&mut native.szAccessPath);
        clear_pwstr(&mut native.szItemID);
        clear_com_blob(&mut native.pBlob);
        clear_variant(&mut native.vEUInfo);
    }
    native.dwBlobSize = 0;
}

pub(crate) unsafe fn clear_item_property(native: &mut crate::bindings::da::tagOPCITEMPROPERTY) {
    // OPC DA requires clients to release the returned strings and clear the
    // VARIANT for every returned property, including per-property failures.
    // SAFETY: These fields belong to the COM-returned property structure.
    unsafe {
        clear_pwstr(&mut native.szItemID);
        clear_pwstr(&mut native.szDescription);
        clear_variant(&mut native.vValue);
    }
}

pub(crate) unsafe fn clear_item_properties(native: &mut crate::bindings::da::tagOPCITEMPROPERTIES) {
    if !native.pItemProperties.is_null() {
        let can_clear_properties = usize::try_from(native.dwNumProperties)
            .ok()
            .and_then(|count| {
                count.checked_mul(core::mem::size_of::<crate::bindings::da::tagOPCITEMPROPERTY>())
            })
            .and_then(|required_bytes| {
                (required_bytes <= isize::MAX as usize).then_some(required_bytes)
            })
            .is_some_and(|required_bytes| {
                // SAFETY: pItemProperties is documented as a COM task allocation.
                unsafe {
                    task_mem_allocation_bytes(native.pItemProperties.cast())
                        .is_some_and(|allocated_bytes| required_bytes <= allocated_bytes)
                }
            });
        if can_clear_properties {
            // SAFETY: OPC DA returns dwNumProperties initialized entries.
            let properties = unsafe {
                core::slice::from_raw_parts_mut(
                    native.pItemProperties,
                    native.dwNumProperties as usize,
                )
            };
            for property in properties {
                // SAFETY: Each nested property is cleared exactly once.
                unsafe { clear_item_property(property) };
            }
        }
        // SAFETY: OPC DA allocates the property array with the COM task allocator.
        unsafe { CoTaskMemFree(Some(native.pItemProperties.cast())) };
        native.pItemProperties = core::ptr::null_mut();
    }
    native.dwNumProperties = 0;
}

pub(crate) unsafe fn clear_browse_element(native: &mut crate::bindings::da::tagOPCBROWSEELEMENT) {
    // SAFETY: These allocations are nested in this COM-returned browse element.
    unsafe {
        clear_pwstr(&mut native.szName);
        clear_pwstr(&mut native.szItemID);
        clear_item_properties(&mut native.ItemProperties);
    }
}

/// Opaque handle for an OPC group.
///
/// This wrapper type enhances type safety when interacting with OPC COM interfaces,
/// preventing accidental mixing of group and item handles.
///
/// # Examples
///
/// ```
/// use opc_da_client::GroupHandle;
/// let handle = GroupHandle(123u32);
/// assert_eq!(handle.0, 123u32);
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GroupHandle(pub u32);

/// Opaque handle for an OPC item.
///
/// Similar to [`GroupHandle`], this ensures type-safe identification of tags
/// within an OPC group.
///
/// # Examples
///
/// ```
/// use opc_da_client::ItemHandle;
/// let handle = ItemHandle(456u32);
/// assert_eq!(handle.0, 456u32);
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ItemHandle(pub u32);

/// Supported OPC DA Specification versions.
#[derive(Debug, Clone, PartialEq)]
pub enum Version {
    V1,
    V2,
    V3,
}

/// Current state and properties of an active OPC group.
///
/// This structure encapsulates both the requested and currently active properties
/// of an OPC group, as reported by the server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupState {
    /// Actual update rate in milliseconds (may differ from requested).
    pub update_rate: u32,
    /// Whether the group is currently active (processing updates).
    pub active: bool,
    /// The unique name of the group.
    pub name: String,
    /// Time zone bias in minutes from UTC.
    pub time_bias: i32,
    /// Percent change for a tag value required to trigger an update.
    pub percent_deadband: f32,
    /// Locale ID used for formatting strings in this group.
    pub locale_id: u32,
    /// Handle assigned by the client for this group.
    pub client_handle: GroupHandle,
    /// Handle assigned by the server for this group.
    pub server_handle: GroupHandle,
}

/// Operational status and metadata of the connected server.
///
/// This structure provides a snapshot of the server's health, current load,
/// and version information.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    /// Time when the server was started.
    pub start_time: std::time::SystemTime,
    /// Current time according to the server.
    pub current_time: std::time::SystemTime,
    /// Time of the last data update.
    pub last_update_time: std::time::SystemTime,
    /// The current operational state of the server.
    pub server_state: ServerState,
    /// Number of groups currently managed by the server.
    pub group_count: u32,
    /// Current bandwidth utilization as reported by the server.
    pub band_width: u32,
    /// Major version of the server software.
    pub major_version: u16,
    /// Minor version of the server software.
    pub minor_version: u16,
    /// Build or revision number of the server software.
    pub build_number: u16,
    /// Descriptive vendor-specific information.
    pub vendor_info: String,
}

impl TryFromNative<crate::bindings::da::tagOPCSERVERSTATUS> for ServerStatus {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCSERVERSTATUS,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            start_time: try_from_native!(&native.ftStartTime),
            current_time: try_from_native!(&native.ftCurrentTime),
            last_update_time: try_from_native!(&native.ftLastUpdateTime),
            server_state: try_from_native!(&native.dwServerState),
            group_count: native.dwGroupCount,
            band_width: native.dwBandWidth,
            major_version: native.wMajorVersion,
            minor_version: native.wMinorVersion,
            build_number: native.wBuildNumber,
            vendor_info: try_from_native!(&native.szVendorInfo),
        })
    }
}

/// Definition required to add a new item to an OPC group.
///
/// This structure contains the parameters needed for the server to identify
/// and initialize a tag within a group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemDef {
    /// Optional access path for the item (server-specific).
    pub access_path: String,
    /// The unique identifier of the tag within the server namespace.
    pub item_id: String,
    /// Whether the item should be added in an active state.
    pub active: bool,
    /// Handle assigned by the client for this item.
    pub client_handle: ItemHandle,
    /// Requested canonical data type (0 for server default).
    pub data_type: u16,
    /// Optional opaque blob for the item.
    pub blob: Vec<u8>,
}

/// FFI-safe bridge struct for `ItemDef`.
pub struct ItemDefBridge {
    pub access_path: LocalPointer<Vec<u16>>,
    pub item_id: LocalPointer<Vec<u16>>,
    pub active: bool,
    pub item_client_handle: u32,
    pub requested_data_type: u16,
    pub blob: LocalPointer<Vec<u8>>,
}

impl IntoBridge<ItemDefBridge> for ItemDef {
    fn into_bridge(self) -> ItemDefBridge {
        ItemDefBridge {
            access_path: LocalPointer::from(&self.access_path),
            item_id: LocalPointer::from(&self.item_id),
            active: self.active,
            item_client_handle: self.client_handle.0,
            requested_data_type: self.data_type,
            blob: LocalPointer::new(Some(self.blob)),
        }
    }
}

impl TryToNative<crate::bindings::da::tagOPCITEMDEF> for ItemDefBridge {
    fn try_to_native(&self) -> windows::core::Result<crate::bindings::da::tagOPCITEMDEF> {
        Ok(crate::bindings::da::tagOPCITEMDEF {
            szAccessPath: self.access_path.as_pwstr(),
            szItemID: self.item_id.as_pwstr(),
            bActive: self.active.into(),
            hClient: self.item_client_handle,
            vtRequestedDataType: self.requested_data_type,
            dwBlobSize: self.blob.len().try_into().map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Blob size exceeds u32 maximum value",
                )
            })?,
            pBlob: self.blob.as_array_ptr() as *mut _,
            wReserved: 0,
        })
    }
}

/// Result properties of an item after being added to a group.
///
/// This structure contains the server-assigned properties for an item
/// that was successfully added.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemResult {
    /// Handle assigned by the server for this item.
    pub server_handle: ItemHandle,
    /// The actual canonical data type supported by the server for this item.
    pub data_type: u16,
    /// Access rights for this item (read/write permissions).
    pub access_rights: u32,
    /// Optional opaque blob returned by the server.
    pub blob: Vec<u8>,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMRESULT> for ItemResult {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMRESULT,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            server_handle: ItemHandle(native.hServer),
            data_type: native.vtCanonicalDataType,
            access_rights: native.dwAccessRights,
            // SAFETY: This conversion borrows the blob. The containing remote
            // item-result array remains responsible for freeing it.
            blob: unsafe { copy_com_slice(native.pBlob, native.dwBlobSize, "item result blob")? },
        })
    }
}

/// Current running state of the OPC server.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    Running,
    Failed,
    NoConfig,
    Suspended,
    Test,
    CommunicationFault,
}

impl TryFromNative<crate::bindings::da::tagOPCSERVERSTATE> for ServerState {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCSERVERSTATE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_STATUS_RUNNING => Ok(ServerState::Running),
            crate::bindings::da::OPC_STATUS_FAILED => Ok(ServerState::Failed),
            crate::bindings::da::OPC_STATUS_NOCONFIG => Ok(ServerState::NoConfig),
            crate::bindings::da::OPC_STATUS_SUSPENDED => Ok(ServerState::Suspended),
            crate::bindings::da::OPC_STATUS_TEST => Ok(ServerState::Test),
            crate::bindings::da::OPC_STATUS_COMM_FAULT => Ok(ServerState::CommunicationFault),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown server state: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCSERVERSTATE> for ServerState {
    fn to_native(&self) -> crate::bindings::da::tagOPCSERVERSTATE {
        match self {
            ServerState::Running => crate::bindings::da::OPC_STATUS_RUNNING,
            ServerState::Failed => crate::bindings::da::OPC_STATUS_FAILED,
            ServerState::NoConfig => crate::bindings::da::OPC_STATUS_NOCONFIG,
            ServerState::Suspended => crate::bindings::da::OPC_STATUS_SUSPENDED,
            ServerState::Test => crate::bindings::da::OPC_STATUS_TEST,
            ServerState::CommunicationFault => crate::bindings::da::OPC_STATUS_COMM_FAULT,
        }
    }
}

/// Scope for enumerating server items or connections.
#[derive(Debug, Clone, PartialEq)]
pub enum EnumScope {
    PrivateConnections,
    PublicConnections,
    AllConnections,
    Public,
    Private,
    All,
}

impl TryFromNative<crate::bindings::da::tagOPCENUMSCOPE> for EnumScope {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCENUMSCOPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_ENUM_PRIVATE_CONNECTIONS => Ok(EnumScope::PrivateConnections),
            crate::bindings::da::OPC_ENUM_PUBLIC_CONNECTIONS => Ok(EnumScope::PublicConnections),
            crate::bindings::da::OPC_ENUM_ALL_CONNECTIONS => Ok(EnumScope::AllConnections),
            crate::bindings::da::OPC_ENUM_PUBLIC => Ok(EnumScope::Public),
            crate::bindings::da::OPC_ENUM_PRIVATE => Ok(EnumScope::Private),
            crate::bindings::da::OPC_ENUM_ALL => Ok(EnumScope::All),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown enum scope: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCENUMSCOPE> for EnumScope {
    fn to_native(&self) -> crate::bindings::da::tagOPCENUMSCOPE {
        match self {
            EnumScope::PrivateConnections => crate::bindings::da::OPC_ENUM_PRIVATE_CONNECTIONS,
            EnumScope::PublicConnections => crate::bindings::da::OPC_ENUM_PUBLIC_CONNECTIONS,
            EnumScope::AllConnections => crate::bindings::da::OPC_ENUM_ALL_CONNECTIONS,
            EnumScope::Public => crate::bindings::da::OPC_ENUM_PUBLIC,
            EnumScope::Private => crate::bindings::da::OPC_ENUM_PRIVATE,
            EnumScope::All => crate::bindings::da::OPC_ENUM_ALL,
        }
    }
}

/// Full attribute set of a single OPC item.
pub struct ItemAttributes {
    pub access_path: String,
    pub item_id: String,
    pub active: bool,
    pub client_handle: ItemHandle,
    pub server_handle: ItemHandle,
    pub access_rights: u32,
    pub blob: Vec<u8>,
    pub requested_data_type: u16,
    pub canonical_data_type: u16,
    pub eu_type: EuType,
    pub eu_info: windows::Win32::System::Variant::VARIANT,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMATTRIBUTES> for ItemAttributes {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMATTRIBUTES,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            access_path: try_from_native!(&native.szAccessPath),
            item_id: try_from_native!(&native.szItemID),
            active: native.bActive.into(),
            client_handle: ItemHandle(native.hClient),
            server_handle: ItemHandle(native.hServer),
            access_rights: native.dwAccessRights,
            // SAFETY: This conversion borrows the blob. The containing remote
            // attributes array remains responsible for freeing it.
            blob: unsafe {
                copy_com_slice(native.pBlob, native.dwBlobSize, "item attributes blob")?
            },
            requested_data_type: native.vtRequestedDataType,
            canonical_data_type: native.vtCanonicalDataType,
            eu_type: try_from_native!(&native.dwEUType),
            eu_info: clone_variant(&native.vEUInfo)?,
        })
    }
}

/// Engineering Units (EU) classification type.
pub enum EuType {
    NoEnum,
    Analog,
    Enumerated,
}

impl TryFromNative<crate::bindings::da::tagOPCEUTYPE> for EuType {
    fn try_from_native(native: &crate::bindings::da::tagOPCEUTYPE) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_NOENUM => Ok(EuType::NoEnum),
            crate::bindings::da::OPC_ANALOG => Ok(EuType::Analog),
            crate::bindings::da::OPC_ENUMERATED => Ok(EuType::Enumerated),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown EU type: {unknown:?}"),
            )),
        }
    }
}

/// Current state of a watched OPC item including value, quality, and time.
pub struct ItemState {
    pub client_handle: ItemHandle,
    pub timestamp: std::time::SystemTime,
    pub quality: u16,
    pub data_value: windows::Win32::System::Variant::VARIANT,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMSTATE> for ItemState {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMSTATE,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            client_handle: ItemHandle(native.hClient),
            timestamp: try_from_native!(&native.ftTimeStamp),
            quality: native.wQuality,
            data_value: clone_variant(&native.vDataValue)?,
        })
    }
}

/// Reading source preference (Cache or Device).
pub enum DataSourceTarget {
    ForceCache,
    ForceDevice,
    WithMaxAge(u32),
}

impl DataSourceTarget {
    pub fn max_age(&self) -> u32 {
        match self {
            DataSourceTarget::WithMaxAge(max_age) => *max_age,
            DataSourceTarget::ForceCache => u32::MAX,
            DataSourceTarget::ForceDevice => 0,
        }
    }
}

impl TryFromNative<crate::bindings::da::tagOPCDATASOURCE> for DataSourceTarget {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCDATASOURCE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_DS_CACHE => Ok(DataSourceTarget::ForceCache),
            crate::bindings::da::OPC_DS_DEVICE => Ok(DataSourceTarget::ForceDevice),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown data source: {unknown:?}"),
            )),
        }
    }
}

impl TryToNative<crate::bindings::da::tagOPCDATASOURCE> for DataSourceTarget {
    fn try_to_native(&self) -> windows::core::Result<crate::bindings::da::tagOPCDATASOURCE> {
        match self {
            DataSourceTarget::ForceCache => Ok(crate::bindings::da::OPC_DS_CACHE),
            DataSourceTarget::ForceDevice => Ok(crate::bindings::da::OPC_DS_DEVICE),
            DataSourceTarget::WithMaxAge(_) => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "MaxAge data source requires a value",
            )),
        }
    }
}

/// Full read value result carrying value, quality, and timestamp.
pub struct ItemValue {
    pub value: windows::Win32::System::Variant::VARIANT,
    pub quality: u16,
    pub timestamp: std::time::SystemTime,
}

impl
    TryFromNative<(
        RemoteArray<windows::Win32::System::Variant::VARIANT>,
        RemoteArray<u16>,
        RemoteArray<windows::Win32::Foundation::FILETIME>,
        RemoteArray<windows::core::HRESULT>,
    )> for Vec<windows::core::Result<ItemValue>>
{
    fn try_from_native(
        native: &(
            RemoteArray<windows::Win32::System::Variant::VARIANT>,
            RemoteArray<u16>,
            RemoteArray<windows::Win32::Foundation::FILETIME>,
            RemoteArray<windows::core::HRESULT>,
        ),
    ) -> windows::core::Result<Self> {
        let (values, qualities, timestamps, errors) = native;
        values.validate_output("item values")?;
        qualities.validate_output("item qualities")?;
        timestamps.validate_output("item timestamps")?;
        errors.validate_output("item errors")?;

        if values.len() != qualities.len()
            || values.len() != timestamps.len()
            || values.len() != errors.len()
        {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "Arrays have different lengths",
            ));
        }

        Ok(values
            .as_slice()
            .iter()
            .zip(qualities.as_slice())
            .zip(timestamps.as_slice())
            .zip(errors.as_slice())
            .map(|(((value, quality), timestamp), error)| {
                if error.is_ok() {
                    Ok(ItemValue {
                        value: clone_variant(value)?,
                        quality: *quality,
                        timestamp: try_from_native!(timestamp),
                    })
                } else {
                    Err((*error).into())
                }
            })
            .collect())
    }
}

/// Item value struct for writes or partial updates.
pub struct ItemPartialValue {
    pub value: windows::Win32::System::Variant::VARIANT,
    pub quality: Option<u16>,
    pub timestamp: Option<std::time::SystemTime>,
}

/// An owning OPC DA value-quality-timestamp input.
///
/// The generated `tagOPCITEMVQT` binding has a shallow `Clone` implementation
/// and no destructor. This wrapper owns the cloned `VARIANT`, prevents shallow
/// copies at the Rust API boundary, and releases the value after the COM call.
#[repr(transparent)]
pub(crate) struct OwnedItemVqt {
    native: crate::bindings::da::tagOPCITEMVQT,
}

impl OwnedItemVqt {
    /// Views a borrowed wrapper slice as the native VQT slice required by COM.
    ///
    /// The returned slice must not outlive `values`; each wrapper remains the
    /// unique owner of its nested `VARIANT`.
    pub(crate) fn as_native_slice(values: &[Self]) -> &[crate::bindings::da::tagOPCITEMVQT] {
        if values.is_empty() {
            return &[];
        }
        // SAFETY: `Self` is repr(transparent) over tagOPCITEMVQT, so the
        // contiguous wrapper slice has identical layout and alignment.
        unsafe {
            core::slice::from_raw_parts(
                values.as_ptr().cast::<crate::bindings::da::tagOPCITEMVQT>(),
                values.len(),
            )
        }
    }
}

impl Drop for OwnedItemVqt {
    fn drop(&mut self) {
        // SAFETY: The wrapper is the sole owner of this cloned VARIANT.
        unsafe { clear_variant(&mut self.native.vDataValue) };
    }
}

impl TryToNative<OwnedItemVqt> for ItemPartialValue {
    fn try_to_native(&self) -> windows::core::Result<OwnedItemVqt> {
        Ok(OwnedItemVqt {
            native: crate::bindings::da::tagOPCITEMVQT {
                vDataValue: clone_variant(&self.value)?,
                bQualitySpecified: self.quality.is_some().into(),
                wQuality: self.quality.unwrap_or_default(),
                bTimeStampSpecified: self.timestamp.is_some().into(),
                ftTimeStamp: self
                    .timestamp
                    .map(|t| t.try_to_native())
                    .transpose()?
                    .unwrap_or_default(),
                wReserved: 0,
                dwReserved: 0,
            },
        })
    }
}

/// Filter type for navigating the namespace (Branch, Leaf, Flat).
pub enum BrowseType {
    Branch,
    Leaf,
    Flat,
}

impl TryFromNative<crate::bindings::da::tagOPCBROWSETYPE> for BrowseType {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCBROWSETYPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_BRANCH => Ok(BrowseType::Branch),
            crate::bindings::da::OPC_LEAF => Ok(BrowseType::Leaf),
            crate::bindings::da::OPC_FLAT => Ok(BrowseType::Flat),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown browse type: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCBROWSETYPE> for BrowseType {
    fn to_native(&self) -> crate::bindings::da::tagOPCBROWSETYPE {
        match self {
            BrowseType::Branch => crate::bindings::da::OPC_BRANCH,
            BrowseType::Leaf => crate::bindings::da::OPC_LEAF,
            BrowseType::Flat => crate::bindings::da::OPC_FLAT,
        }
    }
}

/// Granular filter for enumeration results.
pub enum BrowseFilter {
    All,
    Branches,
    Items,
}

impl TryFromNative<crate::bindings::da::tagOPCBROWSEFILTER> for BrowseFilter {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCBROWSEFILTER,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_BROWSE_FILTER_ALL => Ok(BrowseFilter::All),
            crate::bindings::da::OPC_BROWSE_FILTER_BRANCHES => Ok(BrowseFilter::Branches),
            crate::bindings::da::OPC_BROWSE_FILTER_ITEMS => Ok(BrowseFilter::Items),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown browse filter: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCBROWSEFILTER> for BrowseFilter {
    fn to_native(&self) -> crate::bindings::da::tagOPCBROWSEFILTER {
        match self {
            BrowseFilter::All => crate::bindings::da::OPC_BROWSE_FILTER_ALL,
            BrowseFilter::Branches => crate::bindings::da::OPC_BROWSE_FILTER_BRANCHES,
            BrowseFilter::Items => crate::bindings::da::OPC_BROWSE_FILTER_ITEMS,
        }
    }
}

/// Typology of the server's address space.
pub enum NamespaceType {
    Flat,
    Hierarchy,
}

impl TryFromNative<crate::bindings::da::tagOPCNAMESPACETYPE> for NamespaceType {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCNAMESPACETYPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_NS_HIERARCHIAL => Ok(NamespaceType::Hierarchy),
            crate::bindings::da::OPC_NS_FLAT => Ok(NamespaceType::Flat),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown namespace type: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCNAMESPACETYPE> for NamespaceType {
    fn to_native(&self) -> crate::bindings::da::tagOPCNAMESPACETYPE {
        match self {
            NamespaceType::Hierarchy => crate::bindings::da::OPC_NS_HIERARCHIAL,
            NamespaceType::Flat => crate::bindings::da::OPC_NS_FLAT,
        }
    }
}

// COSERVERINFO
/// Information defining how to connect to a remote server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub auth_info: AuthInfo,
}

/// FFI-safe bridge for `ServerInfo` (COSERVERINFO).
pub struct ServerInfoBridge {
    pub name: LocalPointer<Vec<u16>>,
    pub auth_info: AuthInfoBridge,
}

impl IntoBridge<ServerInfoBridge> for ServerInfo {
    fn into_bridge(self) -> ServerInfoBridge {
        ServerInfoBridge {
            name: LocalPointer::from(&self.name),
            auth_info: self.auth_info.into_bridge(),
        }
    }
}

impl TryToNative<windows::Win32::System::Com::COSERVERINFO> for ServerInfoBridge {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::System::Com::COSERVERINFO> {
        Ok(windows::Win32::System::Com::COSERVERINFO {
            dwReserved1: 0,
            dwReserved2: 0,
            pwszName: self.name.as_pwstr(),
            pAuthInfo: &self.auth_info.try_to_native()? as *const _ as *mut _,
        })
    }
}

/// Authentication and authorization settings for DCOM.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthInfo {
    pub authn_svc: u32,
    pub authz_svc: u32,
    pub server_principal_name: String,
    pub authn_level: u32,
    pub impersonation_level: u32,
    pub auth_identity_data: AuthIdentity,
    pub capabilities: u32,
}

/// FFI-safe bridge for `AuthInfo` (COAUTHINFO).
pub struct AuthInfoBridge {
    pub authn_svc: u32,
    pub authz_svc: u32,
    pub server_principal_name: LocalPointer<Vec<u16>>,
    pub authn_level: u32,
    pub impersonation_level: u32,
    pub auth_identity_data: AuthIdentityBridge,
    pub capabilities: u32,
}

impl IntoBridge<AuthInfoBridge> for AuthInfo {
    fn into_bridge(self) -> AuthInfoBridge {
        AuthInfoBridge {
            authn_svc: self.authn_svc,
            authz_svc: self.authz_svc,
            server_principal_name: LocalPointer::from(&self.server_principal_name),
            authn_level: self.authn_level,
            impersonation_level: self.impersonation_level,
            auth_identity_data: self.auth_identity_data.into_bridge(),
            capabilities: self.capabilities,
        }
    }
}

impl TryToNative<windows::Win32::System::Com::COAUTHINFO> for AuthInfoBridge {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::System::Com::COAUTHINFO> {
        Ok(windows::Win32::System::Com::COAUTHINFO {
            dwAuthnSvc: self.authn_svc,
            dwAuthzSvc: self.authz_svc,
            pwszServerPrincName: self.server_principal_name.as_pwstr(),
            dwAuthnLevel: self.authn_level,
            dwImpersonationLevel: self.impersonation_level,
            pAuthIdentityData: &self.auth_identity_data.try_to_native()? as *const _ as *mut _,
            dwCapabilities: self.capabilities,
        })
    }
}

/// DCOM authentication credentials.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthIdentity {
    pub user: String,
    pub domain: String,
    pub password: String,
    pub flags: u32,
}

/// FFI-safe bridge for `AuthIdentity` (COAUTHIDENTITY).
pub struct AuthIdentityBridge {
    pub user: LocalPointer<Vec<u16>>,
    pub domain: LocalPointer<Vec<u16>>,
    pub password: LocalPointer<Vec<u16>>,
    pub flags: u32,
}

impl IntoBridge<AuthIdentityBridge> for AuthIdentity {
    fn into_bridge(self) -> AuthIdentityBridge {
        AuthIdentityBridge {
            user: LocalPointer::from(&self.user),
            domain: LocalPointer::from(&self.domain),
            password: LocalPointer::from(&self.password),
            flags: self.flags,
        }
    }
}

impl TryToNative<windows::Win32::System::Com::COAUTHIDENTITY> for AuthIdentityBridge {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::System::Com::COAUTHIDENTITY> {
        Ok(windows::Win32::System::Com::COAUTHIDENTITY {
            User: self.user.as_pwstr().0,
            UserLength: self.user.len().try_into().map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "User name exceeds u32 maximum length",
                )
            })?,
            Domain: self.domain.as_pwstr().0,
            DomainLength: self.domain.len().try_into().map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Domain name exceeds u32 maximum length",
                )
            })?,
            Password: self.password.as_pwstr().0,
            PasswordLength: self.password.len().try_into().map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Password exceeds u32 maximum length",
                )
            })?,
            Flags: self.flags,
        })
    }
}

/// COM instantiation context flags (CLSCTX).
#[derive(Debug, Clone, PartialEq)]
pub enum ClassContext {
    All,
    InProcServer,
    InProcHandler,
    LocalServer,
    InProcServer16,
    RemoteServer,
    InProcHandler16,
    NoCodeDownload,
    NoCustomMarshal,
    EnableCodeDownload,
    NoFailureLog,
    DisableAAA,
    EnableAAA,
    FromDefaultContext,
    ActivateX86Server,
    Activate32BitServer,
    Activate64BitServer,
    EnableCloaking,
    AppContainer,
    ActivateAAAAsIU,
    ActivateARM32Server,
    AllowLowerTrustRegistration,
    PsDll,
}

impl ToNative<windows::Win32::System::Com::CLSCTX> for ClassContext {
    fn to_native(&self) -> windows::Win32::System::Com::CLSCTX {
        match self {
            ClassContext::All => windows::Win32::System::Com::CLSCTX_ALL,
            ClassContext::InProcServer => windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            ClassContext::InProcHandler => windows::Win32::System::Com::CLSCTX_INPROC_HANDLER,
            ClassContext::LocalServer => windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
            ClassContext::InProcServer16 => windows::Win32::System::Com::CLSCTX_INPROC_SERVER16,
            ClassContext::RemoteServer => windows::Win32::System::Com::CLSCTX_REMOTE_SERVER,
            ClassContext::InProcHandler16 => windows::Win32::System::Com::CLSCTX_INPROC_HANDLER16,
            ClassContext::NoCodeDownload => windows::Win32::System::Com::CLSCTX_NO_CODE_DOWNLOAD,
            ClassContext::NoCustomMarshal => windows::Win32::System::Com::CLSCTX_NO_CUSTOM_MARSHAL,
            ClassContext::EnableCodeDownload => {
                windows::Win32::System::Com::CLSCTX_ENABLE_CODE_DOWNLOAD
            }
            ClassContext::NoFailureLog => windows::Win32::System::Com::CLSCTX_NO_FAILURE_LOG,
            ClassContext::DisableAAA => windows::Win32::System::Com::CLSCTX_DISABLE_AAA,
            ClassContext::EnableAAA => windows::Win32::System::Com::CLSCTX_ENABLE_AAA,
            ClassContext::FromDefaultContext => {
                windows::Win32::System::Com::CLSCTX_FROM_DEFAULT_CONTEXT
            }
            ClassContext::ActivateX86Server => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_X86_SERVER
            }
            ClassContext::Activate32BitServer => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_32_BIT_SERVER
            }
            ClassContext::Activate64BitServer => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_64_BIT_SERVER
            }
            ClassContext::EnableCloaking => windows::Win32::System::Com::CLSCTX_ENABLE_CLOAKING,
            ClassContext::AppContainer => windows::Win32::System::Com::CLSCTX_APPCONTAINER,
            ClassContext::ActivateAAAAsIU => windows::Win32::System::Com::CLSCTX_ACTIVATE_AAA_AS_IU,
            ClassContext::ActivateARM32Server => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_ARM32_SERVER
            }
            ClassContext::AllowLowerTrustRegistration => {
                windows::Win32::System::Com::CLSCTX_ALLOW_LOWER_TRUST_REGISTRATION
            }
            ClassContext::PsDll => windows::Win32::System::Com::CLSCTX_PS_DLL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Variant::{VT_BSTR, VT_EMPTY};
    use windows::core::{BSTR, PWSTR};

    fn com_wide(value: &str) -> PWSTR {
        let value = value
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: Allocate enough COM task memory for the terminated string.
        let pointer =
            unsafe { windows::Win32::System::Com::CoTaskMemAlloc(core::mem::size_of_val(&*value)) }
                .cast::<u16>();
        assert!(!pointer.is_null());
        // SAFETY: The allocation is large enough and does not overlap value.
        unsafe {
            core::ptr::copy_nonoverlapping(value.as_ptr(), pointer, value.len());
        }
        PWSTR(pointer)
    }

    fn bstr_variant(value: &str) -> windows::Win32::System::Variant::VARIANT {
        let mut variant = windows::Win32::System::Variant::VARIANT::default();
        // SAFETY: The discriminant and matching union field are set together.
        unsafe {
            (*variant.Anonymous.Anonymous).vt = VT_BSTR;
            (*variant.Anonymous.Anonymous).Anonymous.bstrVal =
                core::mem::ManuallyDrop::new(BSTR::from(value));
        }
        variant
    }

    #[test]
    fn item_result_conversion_borrows_blob_until_owner_cleanup() {
        let source = [1_u8, 2, 3];
        // SAFETY: Allocate enough COM task memory for the blob.
        let blob =
            unsafe { windows::Win32::System::Com::CoTaskMemAlloc(source.len()) }.cast::<u8>();
        assert!(!blob.is_null());
        // SAFETY: The allocation is large enough and does not overlap source.
        unsafe {
            core::ptr::copy_nonoverlapping(source.as_ptr(), blob, source.len());
        }
        let mut native = crate::bindings::da::tagOPCITEMRESULT {
            dwBlobSize: u32::try_from(source.len()).unwrap(),
            pBlob: blob,
            ..Default::default()
        };

        let converted = ItemResult::try_from_native(&native).unwrap();

        assert_eq!(converted.blob, source);
        // SAFETY: Conversion borrowed the native blob; its owner can still
        // inspect and then release it exactly once.
        assert_eq!(
            unsafe { core::slice::from_raw_parts(native.pBlob, 3) },
            source
        );
        // SAFETY: native owns its nested COM blob.
        unsafe { clear_item_result(&mut native) };
        assert!(native.pBlob.is_null());
        assert_eq!(native.dwBlobSize, 0);
    }

    #[test]
    fn item_result_rejects_blob_length_larger_than_com_allocation() {
        // SAFETY: Allocate a COM task buffer.
        let blob = unsafe { windows::Win32::System::Com::CoTaskMemAlloc(2) }.cast::<u8>();
        assert!(!blob.is_null());
        let capacity = unsafe { task_mem_allocation_bytes(blob.cast()) }
            .expect("the test allocation must have a measurable COM capacity");
        let invalid_size = u32::try_from(capacity)
            .expect("the test allocator capacity must fit in u32")
            .checked_add(1)
            .expect("the test allocator capacity must fit in u32");
        let mut native = crate::bindings::da::tagOPCITEMRESULT {
            dwBlobSize: invalid_size,
            pBlob: blob,
            ..Default::default()
        };

        let error = ItemResult::try_from_native(&native).unwrap_err();

        assert_eq!(error.code(), windows::Win32::Foundation::E_INVALIDARG);
        // SAFETY: native still owns the blob after the failed conversion.
        unsafe { clear_item_result(&mut native) };
        assert!(native.pBlob.is_null());
    }

    #[test]
    fn item_result_conversion_rejects_nonempty_null_blob() {
        let native = crate::bindings::da::tagOPCITEMRESULT {
            dwBlobSize: 3,
            pBlob: core::ptr::null_mut(),
            ..Default::default()
        };

        let error = ItemResult::try_from_native(&native)
            .expect_err("a nonempty null blob must be rejected");

        assert_eq!(error.code(), windows::Win32::Foundation::E_POINTER);
    }

    #[test]
    fn item_state_conversion_deep_copies_the_variant() {
        let native = crate::bindings::da::tagOPCITEMSTATE {
            ftTimeStamp: std::time::UNIX_EPOCH.try_to_native().unwrap(),
            vDataValue: bstr_variant("value"),
            ..Default::default()
        };

        let converted = ItemState::try_from_native(&native).unwrap();

        assert_eq!(
            crate::helpers::variant_to_string(&converted.data_value),
            "value"
        );
        assert_eq!(
            crate::helpers::variant_to_string(&native.vDataValue),
            "value"
        );
    }

    #[test]
    fn item_partial_value_uses_an_owning_vqt_wrapper() {
        let mut partial = ItemPartialValue {
            value: bstr_variant("value"),
            quality: Some(0xC0),
            timestamp: None,
        };

        let native = partial.try_to_native().unwrap();
        let native_slice = OwnedItemVqt::as_native_slice(std::slice::from_ref(&native));

        assert_eq!(
            crate::helpers::variant_to_string(&native_slice[0].vDataValue),
            "value"
        );
        assert_eq!(native_slice[0].wQuality, 0xC0);
        assert!(native_slice[0].bQualitySpecified.as_bool());
        assert!(!native_slice[0].bTimeStampSpecified.as_bool());

        drop(native);
        assert_eq!(crate::helpers::variant_to_string(&partial.value), "value");
        // SAFETY: The test-created source VARIANT owns its BSTR.
        unsafe { clear_variant(&mut partial.value) };
    }

    #[test]
    fn item_property_cleanup_releases_strings_and_variant() {
        let mut property = crate::bindings::da::tagOPCITEMPROPERTY {
            szItemID: com_wide("item"),
            szDescription: com_wide("description"),
            vValue: bstr_variant("value"),
            ..Default::default()
        };

        // SAFETY: property owns each nested COM allocation.
        unsafe { clear_item_property(&mut property) };

        assert!(property.szItemID.is_null());
        assert!(property.szDescription.is_null());
        assert_eq!(property.vValue.vt(), VT_EMPTY);
    }

    #[test]
    fn failed_item_property_cleanup_releases_returned_fields() {
        let mut property = crate::bindings::da::tagOPCITEMPROPERTY {
            szItemID: com_wide("item"),
            szDescription: com_wide("description"),
            vValue: bstr_variant("value"),
            hrErrorID: windows::Win32::Foundation::E_FAIL,
            ..Default::default()
        };
        // SAFETY: OPC DA requires returned property strings and VARIANTs to
        // be released even when the individual property reports an error.
        unsafe { clear_item_property(&mut property) };

        assert!(property.szItemID.is_null());
        assert!(property.szDescription.is_null());
        assert_eq!(property.vValue.vt(), VT_EMPTY);
    }

    #[test]
    fn malformed_item_property_count_does_not_walk_past_allocation() {
        let pointer = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(core::mem::size_of::<
                crate::bindings::da::tagOPCITEMPROPERTY,
            >())
        }
        .cast::<crate::bindings::da::tagOPCITEMPROPERTY>();
        assert!(!pointer.is_null());
        // SAFETY: The allocation is large enough for one initialized property.
        unsafe { pointer.write(crate::bindings::da::tagOPCITEMPROPERTY::default()) };

        let mut properties = crate::bindings::da::tagOPCITEMPROPERTIES {
            dwNumProperties: 2,
            pItemProperties: pointer,
            ..Default::default()
        };

        // SAFETY: The outer pointer is a COM task allocation. The deliberately
        // inflated count must be rejected before nested cleanup dereferences it.
        unsafe { clear_item_properties(&mut properties) };

        assert!(properties.pItemProperties.is_null());
        assert_eq!(properties.dwNumProperties, 0);
    }
}
