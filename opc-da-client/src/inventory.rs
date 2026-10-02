//! Bounded inventory orchestration on an independent native connection.

mod capabilities;
mod da2;
mod da3;
mod iterator;
mod navigation;
mod progress;
mod state;

use crate::backend::connector::ServerConnector;
use crate::errors::{OpcError, OpcResult, com_hresult, is_da3_browse_compatibility_error};
use crate::inventory::capabilities::capabilities_for_inventory;
use crate::inventory::da2::InventoryPageContext;
use crate::inventory::da3::next_page;
use crate::inventory::progress::{
    merge_warning, progress, record_skipped_invalid_branch, send_event,
};
use crate::inventory::state::{
    BranchLocation, BranchWork, InventoryContinuation, initial_work, is_initial_da3_root,
};
use crate::inventory_boundary::InventoryBoundary;
use crate::inventory_error::InventoryError;
use crate::inventory_telemetry::install_native_operation_telemetry;
use crate::provider::{
    InventoryCompleted, InventoryControl, InventoryEntry, InventoryEvent, InventoryOptions,
    InventorySliceBackend, InventorySliceObservation,
};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub use crate::inventory_telemetry::record_native_operation;

/// Traverse one server, optionally starting at an exact canonical ItemID.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn run_inventory_at_root<C: ServerConnector>(
    connector: &C,
    server_name: &str,
    root_item_id: Option<&str>,
    options: InventoryOptions,
    control: &InventoryControl,
    sender: &mpsc::Sender<OpcResult<InventoryEvent>>,
) -> OpcResult<()> {
    if options.batch_size == 0 || options.batch_size > crate::provider::MAX_INVENTORY_BATCH_SIZE {
        return Err(OpcError::InvalidState(format!(
            "Inventory batch size must be between 1 and {}",
            crate::provider::MAX_INVENTORY_BATCH_SIZE
        )));
    }

    let mut active_time = Duration::ZERO;
    let startup_started = Instant::now();
    tracing::info!(
        server = %server_name,
        batch_size = options.batch_size,
        thread_id = ?std::thread::current().id(),
        "native inventory startup started"
    );
    let connect_started = Instant::now();
    let connected = connector.connect(server_name)?;
    tracing::info!(
        server = %server_name,
        elapsed_ms = connect_started.elapsed().as_millis(),
        total_elapsed_ms = startup_started.elapsed().as_millis(),
        "native inventory server connection completed"
    );
    let mut boundary = InventoryBoundary::new(control);
    let _native_operation_telemetry_scope =
        install_native_operation_telemetry(boundary.telemetry_recorder());
    tracing::info!(server = %server_name, "native inventory capability detection started");
    let capabilities = if control.is_cancelled() {
        crate::provider::BrowseCapabilities {
            namespace: crate::provider::BrowseNamespace::Unknown,
            supports_da3: false,
            supports_da2: false,
            max_page_size: 1_000,
        }
    } else {
        match capabilities_for_inventory(&connected, &mut boundary) {
            Ok(capabilities) => capabilities,
            Err(InventoryError::Cancelled) => crate::provider::BrowseCapabilities {
                namespace: crate::provider::BrowseNamespace::Unknown,
                supports_da3: false,
                supports_da2: false,
                max_page_size: 1_000,
            },
            Err(InventoryError::Failed(error)) => return Err(error),
            Err(InventoryError::InvalidDa2Branch { .. }) => {
                return Err(OpcError::Internal(
                    "invalid DA2 branch escaped capability detection".to_string(),
                ));
            }
        }
    };
    tracing::info!(
        server = %server_name,
        supports_da2 = capabilities.supports_da2,
        supports_da3 = capabilities.supports_da3,
        namespace = ?capabilities.namespace,
        elapsed_ms = startup_started.elapsed().as_millis(),
        "native inventory capability detection completed"
    );
    let startup_native_operation_observations = boundary.take_operation_observations();
    let mut queue = VecDeque::from([initial_work(capabilities, root_item_id)]);
    let mut seen_items = HashSet::new();
    let mut current_da2_path = Vec::new();
    let mut branches_visited = 0_u64;
    let mut entries_seen = 0_u64;
    let mut skipped_invalid_branches = 0_u64;
    let mut first_skipped_invalid_branch = None;
    let mut skipped_non_progressing_branches = 0_u64;
    let mut first_skipped_non_progressing_branch = None;
    let mut slice_sequence = 0_u64;

    let mut terminal = InventoryCompleted {
        complete: true,
        cancelled: false,
        truncated: false,
        warning: None,
        capabilities,
        startup_native_operation_observations,
    };

    if !send_event(
        sender,
        InventoryEvent::Progress(progress(
            branches_visited,
            entries_seen,
            0,
            active_time,
            boundary.paused_time(),
        )),
    ) {
        return Ok(());
    }

    if options.max_entries == Some(0) {
        terminal.complete = false;
        terminal.truncated = true;
        terminal.warning = Some("inventory entry limit reached".to_string());
        let _ = send_event(sender, InventoryEvent::Completed(terminal));
        return Ok(());
    }

    while let Some(mut work) = queue.pop_front() {
        if work.da3_continuation.is_none() && work.da2_state.is_none() {
            branches_visited = branches_visited.saturating_add(1);
        }
        let call_started = Instant::now();
        let paused_before = boundary.paused_time();
        let batch_size = control.batch_size().unwrap_or(options.batch_size);
        let operations_before = boundary.operations();
        let first_native_operation = operations_before == 0;
        let mut page_context = InventoryPageContext {
            current_da2_path: &mut current_da2_path,
            namespace: capabilities.namespace,
            skipped_invalid_branches: &mut skipped_invalid_branches,
            first_skipped_invalid_branch: &mut first_skipped_invalid_branch,
            skipped_non_progressing_branches: &mut skipped_non_progressing_branches,
            first_skipped_non_progressing_branch: &mut first_skipped_non_progressing_branch,
            boundary: &mut boundary,
        };
        let page_result = next_page(&connected, &mut work, batch_size, &mut page_context);
        let slice_elapsed = call_started.elapsed();
        if first_native_operation && boundary.operations() > operations_before {
            tracing::info!(
                server = %server_name,
                elapsed_ms = startup_started.elapsed().as_millis(),
                first_operation_elapsed_ms = slice_elapsed.as_millis(),
                "native inventory first operation completed"
            );
        }
        active_time +=
            slice_elapsed.saturating_sub(boundary.paused_time().saturating_sub(paused_before));
        let page = match page_result {
            Err(InventoryError::Cancelled) => {
                terminal.complete = false;
                terminal.cancelled = true;
                break;
            }
            Ok(page) => page,
            Err(InventoryError::Failed(error)) => {
                if is_initial_da3_root(&work)
                    && terminal.capabilities.supports_da2
                    && is_da3_browse_compatibility_error(&error)
                {
                    let hresult = com_hresult(&error)
                        .map_or_else(|| "N/A".to_string(), |value| format!("0x{value:08X}"));
                    tracing::warn!(
                        hresult = %hresult,
                        error = %error,
                        "OPC DA 3.0 root inventory is incompatible; falling back to OPC DA 2.x"
                    );
                    merge_warning(
                        &mut terminal.warning,
                        format!(
                            "OPC DA 3.0 root browse returned compatibility HRESULT {hresult}; \
                             inventory continued through OPC DA 2.x"
                        ),
                    );
                    terminal.capabilities.supports_da3 = false;
                    branches_visited = branches_visited.saturating_sub(1);
                    queue.clear();
                    queue.push_back(initial_work(terminal.capabilities, root_item_id));
                    current_da2_path.clear();
                    continue;
                }
                let _ = send_event(
                    sender,
                    InventoryEvent::Progress(progress(
                        branches_visited,
                        entries_seen,
                        seen_items.len() as u64,
                        active_time,
                        boundary.paused_time(),
                    )),
                );
                return Err(error);
            }
            Err(InventoryError::InvalidDa2Branch {
                parent_path,
                branch,
            }) => {
                record_skipped_invalid_branch(
                    &mut skipped_invalid_branches,
                    &mut first_skipped_invalid_branch,
                    &parent_path,
                    &branch,
                );
                continue;
            }
        };
        let nodes_returned = page.nodes.len() as u64;
        let has_more = page.continuation.is_some();
        entries_seen = entries_seen.saturating_add(nodes_returned);
        slice_sequence = slice_sequence.saturating_add(1);

        for node in page.nodes {
            if control.is_cancelled() {
                terminal.complete = false;
                terminal.cancelled = true;
                break;
            }

            let display_name = node.display_name;
            if node.kind.is_item()
                && let Some(item_id) = node.item_id.clone()
                && seen_items.insert(item_id.clone())
            {
                if !send_event(
                    sender,
                    InventoryEvent::Entry(InventoryEntry {
                        display_name: display_name.clone(),
                        item_id,
                        kind: node.kind,
                        breadcrumbs: work.breadcrumbs.clone(),
                    }),
                ) {
                    terminal.complete = false;
                    terminal.cancelled = true;
                    break;
                }

                if options
                    .max_entries
                    .is_some_and(|limit| seen_items.len() as u64 >= limit)
                {
                    terminal.complete = false;
                    terminal.truncated = true;
                    merge_warning(
                        &mut terminal.warning,
                        "inventory entry limit reached".to_string(),
                    );
                    break;
                }
            }

            if let Some(location) = node.child {
                let mut breadcrumbs = work.breadcrumbs.clone();
                breadcrumbs.push(display_name);
                queue.push_back(BranchWork {
                    location,
                    breadcrumbs,
                    da3_continuation: None,
                    da3_seen_continuations: HashSet::new(),
                    da3_consecutive_empty_pages: 0,
                    da2_state: None,
                });
            }
        }

        if !send_event(
            sender,
            InventoryEvent::Slice(InventorySliceObservation {
                sequence: slice_sequence,
                backend: match &work.location {
                    BranchLocation::Da3(_) => InventorySliceBackend::Da3,
                    BranchLocation::Da2(_) => InventorySliceBackend::Da2,
                },
                nodes_returned,
                has_more,
                native_operations: boundary.operations().saturating_sub(operations_before),
                native_operation_observations: boundary.take_operation_observations(),
                elapsed_ms: slice_elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                entries_seen,
                unique_items: seen_items.len() as u64,
            }),
        ) {
            terminal.complete = false;
            terminal.cancelled = true;
        }

        if terminal.cancelled || terminal.truncated {
            break;
        }

        if let Some(continuation) = page.continuation {
            match continuation {
                InventoryContinuation::Da3(continuation) => {
                    work.da3_continuation = Some(continuation);
                }
                InventoryContinuation::Da2(state) => {
                    work.da2_state = Some(*state);
                }
            }
            queue.push_front(work);
        }

        if !send_event(
            sender,
            InventoryEvent::Progress(progress(
                branches_visited,
                entries_seen,
                seen_items.len() as u64,
                active_time,
                boundary.paused_time(),
            )),
        ) {
            terminal.complete = false;
            terminal.cancelled = true;
            break;
        }
    }

    if control.is_cancelled() {
        terminal.complete = false;
        terminal.cancelled = true;
    }
    if skipped_invalid_branches > 0 {
        let warning = format!(
            "skipped {skipped_invalid_branches} non-navigable DA2 branch name(s); \
             first skipped branch: {}",
            first_skipped_invalid_branch
                .as_deref()
                .unwrap_or("<unknown>")
        );
        merge_warning(&mut terminal.warning, warning);
    }
    if skipped_non_progressing_branches > 0 {
        let warning = format!(
            "skipped {skipped_non_progressing_branches} non-progressing DA2 branch iterator(s); \
             first skipped iterator: {}",
            first_skipped_non_progressing_branch
                .as_deref()
                .unwrap_or("<unknown>")
        );
        merge_warning(&mut terminal.warning, warning);
    }
    let _ = send_event(sender, InventoryEvent::Completed(terminal));
    Ok(())
}

#[cfg(test)]
#[path = "tests/inventory.rs"]
mod tests;
