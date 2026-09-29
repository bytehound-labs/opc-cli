use windows_core::Interface as _;

use crate::opc_da::{
    client::GuidIterator,
    com_utils::{IntoBridge, ToNative, TryToNative as _},
    errors::{OpcError, OpcResult},
    typedefs::{ClassContext, ServerInfo},
};

/// Trait defining client functionality for OPC Data Access servers.
pub trait ClientTrait<Server: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> {
    /// GUID of the catalog used to enumerate servers.
    const CATALOG_ID: windows::core::GUID;

    /// Retrieves an iterator over available server GUIDs.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `GuidIterator` over server GUIDs, or an error if the operation fails.
    fn get_servers(&self) -> OpcResult<GuidIterator> {
        tracing::debug!("Enumerating OPC DA Server classes via COM Component Categories Manager");
        // SAFETY: `w!` supplies a static, NUL-terminated UTF-16 ProgID; the API writes its
        // result into a local GUID, and `?` checks the HRESULT.
        let id = unsafe {
            windows::Win32::System::Com::CLSIDFromProgID(windows::core::w!("OPC.ServerList.1"))?
        };

        // SAFETY: `id` is a live local CLSID and `None` requests no aggregation. COM must be
        // initialized on this thread (the normal client path holds `ComGuard` on its worker);
        // `?` checks the HRESULT before receiving the interface.
        let servers: crate::bindings::comn::IOPCServerList = unsafe {
            // TODO: Use CoCreateInstanceEx
            windows::Win32::System::Com::CoCreateInstance(
                &id,
                None,
                // TODO: Convert from filters
                windows::Win32::System::Com::CLSCTX_ALL,
            )?
        };

        let versions = [Self::CATALOG_ID];

        // SAFETY: Both category slices reference the live `versions` array for this call;
        // `servers` owns the COM interface, and `?` checks the HRESULT before wrapping the enumerator.
        let iter = unsafe {
            servers
                .EnumClassesOfCategories(&versions, &versions)
                .map_err(|e| {
                    windows::core::Error::new(e.code(), "Failed to enumerate server classes")
                })?
        };

        Ok(GuidIterator::new(iter))
    }

    /// Creates a server instance from the specified class ID.
    ///
    /// # Parameters
    ///
    /// - `class_id`: The GUID of the server class to instantiate.
    ///
    /// # Returns
    ///
    /// A `Result` containing the server instance, or an error if creation fails.
    fn create_server(
        &self,
        class_id: windows::core::GUID,
        class_context: ClassContext,
    ) -> OpcResult<Server> {
        tracing::debug!(
            ?class_id,
            ?class_context,
            "Creating OPC server instance via COM CoCreateInstance"
        );
        // SAFETY: `class_id` is a live local GUID and the class context is passed by value.
        // COM must be initialized on this thread (the normal client path holds `ComGuard` on
        // its worker), and `?` checks the HRESULT.
        let server: crate::bindings::da::IOPCServer = unsafe {
            windows::Win32::System::Com::CoCreateInstance(
                &class_id,
                None,
                class_context.to_native(),
            )?
        };

        server
            .cast::<windows::core::IUnknown>()?
            .try_into()
            .map_err(|source| OpcError::Com { source })
    }

    fn create_server2(
        &self,
        class_id: windows::core::GUID,
        class_context: ClassContext,
        server_info: Option<ServerInfo>,
    ) -> OpcResult<Server> {
        let mut results = [windows::Win32::System::Com::MULTI_QI {
            pIID: &windows::core::IUnknown::IID,
            pItf: core::mem::ManuallyDrop::new(None),
            hr: windows::core::HRESULT(0),
        }];

        // SAFETY: `class_id`, the `MULTI_QI` output array, its static IID, and optional server
        // info remain valid through the call; `?` checks the HRESULT before the per-interface
        // HRESULT and returned pointer are inspected.
        unsafe {
            windows::Win32::System::Com::CoCreateInstanceEx(
                &class_id,
                None,
                class_context.to_native(),
                match server_info {
                    Some(info) => Some(&info.into_bridge().try_to_native()?),
                    None => None,
                },
                &mut results,
            )?
        };

        if results[0].hr.is_err() {
            return Err(OpcError::Com {
                source: results[0].hr.into(),
            });
        }

        match results[0].pItf.as_ref() {
            Some(itf) => itf
                .cast::<windows::core::IUnknown>()?
                .try_into()
                .map_err(|source| OpcError::Com { source }),
            None => Err(OpcError::Com {
                source: windows::core::Error::from(windows::Win32::Foundation::E_POINTER),
            }),
        }
    }
}
