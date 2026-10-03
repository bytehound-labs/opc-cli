//! Native write operation and group cleanup.

use crate::backend::connector::{ConnectedGroup, ConnectedServer, ServerConnector};
use crate::bindings::da::tagOPCITEMDEF;
use crate::com_worker::ComWorker;
use crate::errors::{OpcError, OpcResult};
use crate::helpers::{format_hresult, opc_value_to_variant};
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use crate::provider::{OpcValue, WriteResult};

impl<C: ServerConnector + 'static> ComWorker<C> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_write(
        server_name: &str,
        tag_id: &str,
        value: &OpcValue,
        opc_server: &C::Server,
    ) -> OpcResult<WriteResult> {
        let span = tracing::info_span!(target: "opc_da_client::com_worker",
            "opc.write_tag_value",
            server = %server_name,
            tag = %tag_id
        );
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(target: "opc_da_client::com_worker",
            server = %server_name,
            tag = %tag_id,
            value = ?value,
            "write_tag_value: starting operation"
        );
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-write",
            true,
            1000,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let mut item_id_wide: Vec<u16> = tag_id.encode_utf16().chain(std::iter::once(0)).collect();
        let item_def = tagOPCITEMDEF {
            szAccessPath: windows::core::PWSTR::null(),
            szItemID: windows::core::PWSTR(item_id_wide.as_mut_ptr()),
            bActive: windows::Win32::Foundation::TRUE,
            hClient: 0,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
            vtRequestedDataType: 0,
            wReserved: 0,
        };

        let (results, errors) = group.add_items(&[item_def])?;
        let item_res = results
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item results".to_string()))?;
        let item_err = errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item errors".to_string()))?;

        if let Err(e) = item_err.ok() {
            tracing::warn!(target: "opc_da_client::com_worker", error = ?e, "write_tag_value: failed to add tag to group");
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(target: "opc_da_client::com_worker", error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
            }
            return Ok(WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(format!("Failed to add tag: {}", format_hresult(*item_err))),
            });
        }

        let item_handle = ItemHandle(item_res.hServer);
        let variant = opc_value_to_variant(value);

        let write_errors = group.write(&[item_handle], &[variant])?;
        let write_err = write_errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty write errors".to_string()))?;

        let write_result = if write_err.is_ok() {
            tracing::debug!(target: "opc_da_client::com_worker",
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value completed"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: true,
                error: None,
            }
        } else {
            let msg = format_hresult(*write_err);
            tracing::warn!(target: "opc_da_client::com_worker",
                error = %msg,
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value: server rejected write"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(msg),
            }
        };

        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(target: "opc_da_client::com_worker", error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
        }
        Ok(write_result)
    }
}
