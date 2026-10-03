//! Progress, cumulative warnings, and event delivery.

use crate::errors::OpcResult;
use crate::provider::{InventoryEvent, InventoryProgress};
use std::time::Duration;
use tokio::sync::mpsc;

pub(super) fn merge_warning(existing: &mut Option<String>, warning: String) {
    match existing {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&warning);
        }
        None => *existing = Some(warning),
    }
}

pub(super) fn record_skipped_invalid_branch(
    skipped_invalid_branches: &mut u64,
    first_skipped_invalid_branch: &mut Option<String>,
    parent_path: &[String],
    branch: &str,
) {
    *skipped_invalid_branches = skipped_invalid_branches.saturating_add(1);
    if first_skipped_invalid_branch.is_none() {
        *first_skipped_invalid_branch = Some(format!(
            "name {branch:?} at {}",
            describe_browse_path(parent_path)
        ));
    }
    tracing::warn!(target: "opc_da_client::inventory",
        browse_path = ?parent_path,
        item_name = ?branch,
        hresult = "0x80070057",
        "skipping non-navigable DA2 branch during deferred expansion"
    );
}

pub(super) fn describe_browse_path(path: &[String]) -> String {
    if path.is_empty() {
        "<root>".to_string()
    } else {
        path.iter()
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

#[allow(clippy::cast_precision_loss)]
pub(super) fn progress(
    branches_visited: u64,
    entries_seen: u64,
    unique_items: u64,
    active_time: Duration,
    paused_time: Duration,
) -> InventoryProgress {
    let seconds = active_time.as_secs_f64();
    InventoryProgress {
        branches_visited,
        entries_seen,
        unique_items,
        active_time_ms: active_time.as_millis().try_into().unwrap_or(u64::MAX),
        paused_time_ms: paused_time.as_millis().try_into().unwrap_or(u64::MAX),
        items_per_second: if seconds > 0.0 {
            unique_items as f64 / seconds
        } else {
            0.0
        },
        estimated_remaining_ms: None,
    }
}

pub(super) fn send_event(
    sender: &mpsc::Sender<OpcResult<InventoryEvent>>,
    event: InventoryEvent,
) -> bool {
    sender.blocking_send(Ok(event)).is_ok()
}
