//! Bounded recursive compatibility browse and cursor restoration.

use crate::backend::connector::{ConnectedServer, ServerConnector, guard_browse_iterator};
use crate::bindings::da::{OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_LEAF, OPC_NS_FLAT};
use crate::com_worker::ComWorker;
use crate::errors::{OpcResult, contextual_browse_error, is_non_progress_browse_error};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

impl<C: ServerConnector + 'static> ComWorker<C> {
    pub(super) fn handle_browse(
        server_name: &str,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<String>> {
        let span = tracing::info_span!(target: "opc_da_client::com_worker", "opc.browse_tags", server = %server_name, max_tags);
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(target: "opc_da_client::com_worker",
            server = %server_name,
            max_tags,
            "browse_tags: starting operation"
        );
        let start = std::time::Instant::now();

        let org = opc_server.query_organization()?;
        let mut tags = Vec::new();

        if org == OPC_NS_FLAT.0 as u32 {
            let mut string_iter = guard_browse_iterator(
                opc_server.begin_da2_browse(OPC_LEAF.0 as u32, Some(""), 0, 0)?,
                "recursive flat iterator",
                &[],
            );
            while let Some(tag_res) = string_iter.next_string() {
                if tags.len() >= max_tags {
                    break;
                }
                let tag = tag_res?;
                tags.push(tag.clone());
                if let Ok(mut sink) = tags_sink.lock() {
                    sink.push(tag);
                }
                progress.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            Self::browse_recursive(
                opc_server,
                &mut tags,
                max_tags,
                progress,
                tags_sink,
                &mut Vec::new(),
                0,
            )?;
        }
        tracing::debug!(target: "opc_da_client::com_worker",
            count = tags.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "browse_tags completed"
        );
        Ok(tags)
    }

    pub(super) fn browse_recursive(
        server: &C::Server,
        tags: &mut Vec<String>,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        browse_path: &mut Vec<String>,
        depth: usize,
    ) -> OpcResult<()> {
        const MAX_DEPTH: usize = 50;
        if depth > MAX_DEPTH || tags.len() >= max_tags {
            if depth > MAX_DEPTH {
                tracing::warn!(target: "opc_da_client::com_worker", depth, "Max browse depth reached, truncating");
            }
            return Ok(());
        }

        let mut branch_enum = guard_browse_iterator(
            server.begin_da2_browse(OPC_BRANCH.0 as u32, Some(""), 0, 0)?,
            "recursive branch iterator",
            browse_path,
        );
        let mut branches = Vec::new();
        while let Some(result) = branch_enum.next_string() {
            match result {
                Ok(name) => branches.push(name),
                Err(error) if is_non_progress_browse_error(&error) => {
                    return Err(contextual_browse_error(
                        error,
                        "browse_recursive(branches)",
                        browse_path,
                        None,
                    ));
                }
                Err(e) => {
                    tracing::warn!(target: "opc_da_client::com_worker", error = ?e, "Branch iteration error, skipping");
                }
            }
        }

        let mut leaf_enum = guard_browse_iterator(
            server.begin_da2_browse(OPC_LEAF.0 as u32, Some(""), 0, 0)?,
            "recursive leaf iterator",
            browse_path,
        );
        while let Some(tag_res) = leaf_enum.next_string() {
            if tags.len() >= max_tags {
                return Ok(());
            }
            let browse_name = tag_res?;
            let tag = match server.get_item_id(&browse_name) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(target: "opc_da_client::com_worker",
                        browse_name = %browse_name,
                        error = ?e,
                        "get_item_id failed, using browse name as fallback"
                    );
                    browse_name
                }
            };
            tags.push(tag.clone());
            if let Ok(mut sink) = tags_sink.lock() {
                sink.push(tag);
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }

        for branch in branches {
            if tags.len() >= max_tags {
                return Ok(());
            }
            if let Err(e) = server.change_browse_position(OPC_BROWSE_DOWN.0 as u32, &branch) {
                tracing::warn!(target: "opc_da_client::com_worker",
                    branch = %branch,
                    error = ?e,
                    "Failed to browse down, skipping branch"
                );
                continue;
            }

            browse_path.push(branch.clone());
            let recurse_result = Self::browse_recursive(
                server,
                tags,
                max_tags,
                progress,
                tags_sink,
                browse_path,
                depth + 1,
            );

            let up_result = server.change_browse_position(OPC_BROWSE_UP.0 as u32, "");
            browse_path.pop();

            if let Err(e) = recurse_result {
                if is_non_progress_browse_error(&e) {
                    return Err(e);
                }
                tracing::warn!(target: "opc_da_client::com_worker", error = ?e, "browse_recursive error");
            }

            if let Err(e) = up_result {
                tracing::warn!(target: "opc_da_client::com_worker", error = ?e, "Failed to browse up, stopping recursion");
                break;
            }
        }

        Ok(())
    }
}
