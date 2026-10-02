//! Native read operation and group cleanup.

use crate::backend::connector::{ConnectedGroup, ConnectedServer, ServerConnector};
use crate::bindings::da::{OPC_DS_DEVICE, tagOPCITEMDEF};
use crate::com_worker::{ComWorker, ReadPresentation};
use crate::errors::{OpcError, OpcResult};
use crate::helpers::{
    filetime_to_string, format_hresult, quality_to_string, variant_to_display_string,
    variant_to_string,
};
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use crate::provider::TagValue;

impl<C: ServerConnector + 'static> ComWorker<C> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_read(
        server_name: &str,
        tag_ids: &[String],
        presentation: ReadPresentation,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<TagValue>> {
        let span = tracing::info_span!(target: "opc_da_client::com_worker",
            "opc.read_tag_values",
            server = %server_name,
            tag_count = tag_ids.len()
        );
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(target: "opc_da_client::com_worker",
            server = %server_name,
            tag_count = tag_ids.len(),
            sample_tags = ?tag_ids.iter().take(5).collect::<Vec<_>>(),
            "read_tag_values: starting operation"
        );
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-read",
            true,
            1000,
            server_handle,
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = tag_ids
            .iter()
            .map(|tag_id| tag_id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();

        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;

        // RemoteArray::len() returns u32; tag_ids.len() returns usize.
        if results.len() as usize != tag_ids.len() || errors.len() as usize != tag_ids.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(target: "opc_da_client::com_worker", error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        let mut tag_values: Vec<TagValue> = tag_ids
            .iter()
            .map(|tag_id| TagValue {
                tag_id: tag_id.clone(),
                value: "Error".to_string(),
                quality: "Bad — not added to group".to_string(),
                timestamp: String::new(),
            })
            .collect();

        let mut server_handles: Vec<ItemHandle> = Vec::new();
        let mut valid_indices = Vec::new();

        for (idx, (item_result, error)) in results
            .as_slice()
            .iter()
            .zip(errors.as_slice().iter())
            .enumerate()
        {
            if error.is_ok() {
                server_handles.push(ItemHandle(item_result.hServer));
                valid_indices.push(idx);
            } else {
                let hint = format_hresult(*error);
                tracing::warn!(target: "opc_da_client::com_worker",
                    tag = %tag_ids[idx],
                    error = %hint,
                    "read_tag_values: add_items rejected tag"
                );
                tag_values[idx].quality = format!("Bad — {hint}");
            }
        }

        if server_handles.is_empty() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(target: "opc_da_client::com_worker", error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Ok(tag_values);
        }

        let (item_states, read_errors) = group.read(OPC_DS_DEVICE, &server_handles)?;
        let item_states_slice = item_states.as_slice();
        let read_errors_slice = read_errors.as_slice();

        for (i, idx) in valid_indices.iter().enumerate() {
            let state = &item_states_slice[i];
            let read_error = &read_errors_slice[i];

            let (value_str, quality_str) = if read_error.is_ok() {
                (
                    match presentation {
                        ReadPresentation::Semantic => variant_to_string(&state.vDataValue),
                        ReadPresentation::Display => variant_to_display_string(&state.vDataValue),
                    },
                    quality_to_string(state.wQuality),
                )
            } else {
                let full_msg = format_hresult(*read_error);
                tracing::warn!(target: "opc_da_client::com_worker",
                    tag = %tag_ids[*idx],
                    error = ?read_error,
                    hint = %full_msg,
                    "read_tag_values: per-item read error"
                );
                ("Error".to_string(), format!("Bad — {full_msg}"))
            };

            tag_values[*idx] = TagValue {
                tag_id: tag_ids[*idx].clone(),
                value: value_str,
                quality: quality_str,
                timestamp: filetime_to_string(state.ftTimeStamp),
            };
        }

        tracing::debug!(target: "opc_da_client::com_worker",
            count = tag_values.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "read_tag_values completed"
        );
        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(target: "opc_da_client::com_worker", error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
        }
        Ok(tag_values)
    }
}
