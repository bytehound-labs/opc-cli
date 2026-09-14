//! Feature-gated, read-only OPC DA diagnostics.
//!
//! This module deliberately uses the same connector, server, group, native
//! conversion helpers, and normal worker read path as the production client.
//! It never writes values and never substitutes a cache read for a failed
//! device read.

use crate::backend::connector::{
    ComConnector, ConnectedGroup, ConnectedServer, DiagnosticItemProperty, DiagnosticServerStatus,
    ServerConnector, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE,
};
use crate::bindings::da::{OPC_DS_CACHE, OPC_DS_DEVICE, tagOPCDATASOURCE};
use crate::helpers::{filetime_to_string, quality_to_string, variant_to_string};
use crate::opc_da::com_utils::RemoteArray;
use crate::opc_da::errors::{OpcError, OpcResult};
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use crate::provider::{
    BrowseNodeKind, InventoryCompleted, InventoryEvent, InventoryOptions, InventoryPacing,
    InventoryWorkerJoin, OpcProvider, TagValue,
};
use crate::{ComGuard, OpcDaClient};
use serde::Serialize;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime};
use windows::core::HRESULT;

const YOKOGAWA_PROBE_HRESULT: HRESULT = HRESULT(0xC004_800B_u32.cast_signed());
const MIN_UPDATE_RATE_MS: u32 = 1;
const MAX_UPDATE_RATE_MS: u32 = 60_000;
const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);
const MAX_DEADLINE: Duration = Duration::from_secs(300);
const DEFAULT_INVENTORY_BATCH_SIZE: u32 = 25;
const DEFAULT_INVENTORY_MAX_ENTRIES: u64 = 100;
const DEFAULT_INVENTORY_MIN_INTERVAL: Duration = Duration::from_millis(25);
const DEFAULT_INVENTORY_DEADLINE: Duration = Duration::from_secs(60);
const MAX_INVENTORY_DEADLINE: Duration = Duration::from_secs(600);
const INVENTORY_CANCEL_GRACE: Duration = Duration::from_secs(5);
const DISTANT_INVENTORY_TIMER: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Inputs for one native read-only canary run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeReadCanaryConfig {
    /// OPC DA server ProgID.
    pub prog_id: String,
    /// Exact ItemIDs to validate, add, inspect, and read.
    pub item_ids: Vec<String>,
    /// Requested active-group update interval.
    pub requested_update_rate_ms: u32,
    /// Maximum wall-clock duration for the complete diagnostic.
    pub deadline: Duration,
    /// Item compared with the normal [`OpcDaClient`] worker path.
    ///
    /// When omitted, the first ItemID is used.
    pub worker_compare_item: Option<String>,
}

impl NativeReadCanaryConfig {
    /// Creates a canary configuration with a one-second requested update rate.
    pub fn new(prog_id: impl Into<String>, item_ids: Vec<String>) -> Self {
        Self {
            prog_id: prog_id.into(),
            item_ids,
            requested_update_rate_ms: 1_000,
            deadline: DEFAULT_DEADLINE,
            worker_compare_item: None,
        }
    }

    /// Validates user-controlled inputs before any COM work starts.
    pub fn validate(&self) -> OpcResult<()> {
        validate_config(self)
    }
}

/// Inputs for one bounded, native namespace inventory canary run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInventoryCanaryConfig {
    /// OPC DA server ProgID.
    pub prog_id: String,
    /// Optional DA2 path to use instead of the namespace root.
    pub start_path: Option<Vec<String>>,
    /// Bounded native page size.
    pub options: InventoryOptions,
    /// Pacing applied before each bounded native operation.
    pub pacing: InventoryPacing,
    /// Maximum wall-clock duration for the canary.
    pub deadline: Duration,
    /// Optional delay before requesting cancellation.
    pub cancel_after: Option<Duration>,
}

impl NativeInventoryCanaryConfig {
    /// Creates a conservative bounded inventory configuration.
    pub fn new(prog_id: impl Into<String>) -> Self {
        Self {
            prog_id: prog_id.into(),
            start_path: None,
            options: InventoryOptions {
                batch_size: DEFAULT_INVENTORY_BATCH_SIZE,
                max_entries: Some(DEFAULT_INVENTORY_MAX_ENTRIES),
            },
            pacing: InventoryPacing {
                min_interval: DEFAULT_INVENTORY_MIN_INTERVAL,
                item_rate_per_second: None,
            },
            deadline: DEFAULT_INVENTORY_DEADLINE,
            cancel_after: None,
        }
    }

    /// Validates user-controlled inputs before opening a native inventory.
    pub fn validate(&self) -> OpcResult<()> {
        if self.prog_id.trim().is_empty() {
            return Err(OpcError::InvalidState(
                "inventory diagnostic ProgID cannot be empty".to_string(),
            ));
        }
        if let Some(path) = &self.start_path {
            if path.is_empty() {
                return Err(OpcError::InvalidState(
                    "inventory diagnostic start path cannot be empty".to_string(),
                ));
            }
            for component in path {
                if component.trim().is_empty() {
                    return Err(OpcError::InvalidState(
                        "inventory diagnostic start path components cannot be empty".to_string(),
                    ));
                }
                if component.starts_with('-') {
                    return Err(OpcError::InvalidState(format!(
                        "inventory diagnostic start path component cannot be an option: {component:?}"
                    )));
                }
                if component.contains('\0') {
                    return Err(OpcError::InvalidState(
                        "inventory diagnostic start path components cannot contain NUL".to_string(),
                    ));
                }
            }
        }
        if self.options.batch_size == 0
            || self.options.batch_size > crate::provider::MAX_INVENTORY_BATCH_SIZE
        {
            return Err(OpcError::InvalidState(format!(
                "inventory diagnostic batch size must be between 1 and {}",
                crate::provider::MAX_INVENTORY_BATCH_SIZE
            )));
        }
        if self.options.max_entries == Some(0) {
            return Err(OpcError::InvalidState(
                "inventory diagnostic max entries must be greater than 0".to_string(),
            ));
        }
        if self.pacing.item_rate_per_second == Some(0) {
            return Err(OpcError::InvalidState(
                "inventory diagnostic item rate must be greater than 0".to_string(),
            ));
        }
        if self.deadline.is_zero() || self.deadline > MAX_INVENTORY_DEADLINE {
            return Err(OpcError::InvalidState(format!(
                "inventory diagnostic deadline must be between 1 ms and {MAX_INVENTORY_DEADLINE:?}"
            )));
        }
        if let Some(cancel_after) = self.cancel_after
            && (cancel_after.is_zero() || cancel_after >= self.deadline)
        {
            return Err(OpcError::InvalidState(
                "inventory diagnostic cancellation delay must be greater than 0 and less than the deadline"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// A captured diagnostic call that can retain a non-fatal error in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Captured<T> {
    /// Successful value, when available.
    pub value: Option<T>,
    /// Error text when the call failed.
    pub error: Option<String>,
}

impl<T> Captured<T> {
    fn from_result<E: std::fmt::Display>(result: Result<T, E>) -> Self {
        match result {
            Ok(value) => Self {
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                value: None,
                error: Some(error.to_string()),
            },
        }
    }
}

/// Server status fields returned by `IOPCServer::GetStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerStatusSnapshot {
    pub start_time: String,
    pub current_time: String,
    pub last_update_time: String,
    pub state: String,
    pub group_count: u32,
    pub bandwidth: u32,
    pub version: String,
    pub vendor_info: String,
}

/// Locale and vendor error-string observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerCommonSnapshot {
    pub locale_id: Captured<u32>,
    pub available_locale_ids: Captured<Vec<u32>>,
    pub yokogawa_error_probe: HResultSnapshot,
}

/// HRESULT details from Windows and, when available, the OPC server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HResultSnapshot {
    pub code: i32,
    pub hex: String,
    pub succeeded: bool,
    pub normal_text: String,
    pub vendor_text: Option<String>,
    pub vendor_lookup_error: Option<String>,
}

/// Native metadata returned while validating or adding an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemMetadata {
    pub canonical_data_type: u16,
    pub access_rights: u32,
    pub server_handle: u32,
}

/// Per-item validation and add results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemRegistration {
    pub item_id: String,
    pub validation: HResultSnapshot,
    pub validation_metadata: Option<ItemMetadata>,
    pub addition: HResultSnapshot,
    pub added_metadata: Option<ItemMetadata>,
}

/// One standard OPC item property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemPropertySnapshot {
    pub item_id: String,
    pub property_id: u32,
    pub description: String,
    pub data_type: u16,
    pub value: Option<String>,
    pub result: HResultSnapshot,
}

/// Explicit OPC DA synchronous read source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    Device,
    Cache,
}

impl ReadSource {
    fn native(self) -> tagOPCDATASOURCE {
        match self {
            Self::Device => OPC_DS_DEVICE,
            Self::Cache => OPC_DS_CACHE,
        }
    }
}

/// Timing point relative to the server-revised group update interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadPhase {
    Immediate,
    AfterOneInterval,
    AfterTwoIntervals,
}

/// Safely mapped value, quality, timestamp, and per-item HRESULT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectReadItem {
    pub item_id: String,
    pub result: HResultSnapshot,
    pub value: Option<String>,
    pub quality: Option<String>,
    pub raw_quality: Option<u16>,
    pub timestamp: Option<String>,
}

/// One native synchronous read batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectReadBatch {
    pub phase: ReadPhase,
    pub source: ReadSource,
    pub items: Vec<DirectReadItem>,
    pub error: Option<String>,
}

/// Worker-path value projected into a serializable diagnostic shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerReadValue {
    pub item_id: String,
    pub value: String,
    pub quality: String,
    pub timestamp: String,
}

/// Comparison between one immediate native device read and the normal worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerComparison {
    pub item_id: String,
    pub direct: Option<WorkerReadValue>,
    pub worker: Captured<WorkerReadValue>,
    pub same_value: Option<bool>,
    pub same_quality: Option<bool>,
    pub same_timestamp: Option<bool>,
}

/// Complete structured result of the native canary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReadCanaryReport {
    pub prog_id: String,
    pub requested_update_rate_ms: u32,
    pub revised_update_rate_ms: u32,
    pub status: Captured<ServerStatusSnapshot>,
    pub common: ServerCommonSnapshot,
    pub items: Vec<ItemRegistration>,
    pub properties: Vec<ItemPropertySnapshot>,
    pub property_errors: Vec<Captured<String>>,
    pub reads: Vec<DirectReadBatch>,
    pub cleanup: Captured<bool>,
    pub direct_error: Option<String>,
    pub worker_comparison: Option<WorkerComparison>,
}

/// Runs the read-only canary and compares one value through the normal worker.
pub async fn run_native_read_canary(
    config: NativeReadCanaryConfig,
) -> OpcResult<NativeReadCanaryReport> {
    config.validate()?;
    let deadline = config.deadline;
    match tokio::time::timeout(deadline, run_native_read_canary_inner(config)).await {
        Ok(result) => result,
        Err(_) => Err(OpcError::Internal(format!(
            "native diagnostic exceeded its {deadline:?} deadline; native blocking work may still be running"
        ))),
    }
}

async fn run_native_read_canary_inner(
    config: NativeReadCanaryConfig,
) -> OpcResult<NativeReadCanaryReport> {
    let direct_config = config.clone();
    let mut report = tokio::task::spawn_blocking(move || {
        let _guard = ComGuard::new().map_err(|error| OpcError::Internal(error.to_string()))?;
        run_direct_canary(&ComConnector, &direct_config)
    })
    .await??;

    let comparison_item = config
        .worker_compare_item
        .clone()
        .unwrap_or_else(|| config.item_ids[0].clone());
    let worker = match OpcDaClient::new(ComConnector) {
        Ok(client) => Captured::from_result(
            client
                .read_tag_values(&config.prog_id, vec![comparison_item.clone()])
                .await
                .map(|values| values.into_iter().next())
                .and_then(|value| {
                    value.ok_or_else(|| {
                        OpcError::Internal("worker returned no value for comparison".to_string())
                    })
                })
                .map(worker_value),
        ),
        Err(error) => Captured::from_result(Err::<WorkerReadValue, _>(error)),
    };
    report.worker_comparison = Some(compare_worker_read(&report, &comparison_item, worker));
    Ok(report)
}

/// Runs a bounded native inventory and writes lifecycle records as JSON Lines.
///
/// This diagnostic deliberately reports a channel close without a terminal
/// completion or stream error as a failure. A deadline first requests
/// cancellation, then allows a short grace period for the worker to reach its
/// next bounded native-operation boundary.
pub async fn run_native_inventory_canary(
    config: NativeInventoryCanaryConfig,
    mut output: impl Write,
) -> OpcResult<()> {
    config.validate()?;
    run_native_inventory_canary_inner(config, &mut output).await
}

#[allow(clippy::too_many_lines)]
async fn run_native_inventory_canary_inner(
    config: NativeInventoryCanaryConfig,
    output: &mut impl Write,
) -> OpcResult<()> {
    let started_at = Instant::now();
    write_inventory_json_line(
        output,
        "inventory_start",
        &serde_json::json!({
            "prog_id": config.prog_id,
            "start_path": config.start_path,
            "batch_size": config.options.batch_size,
            "max_entries": config.options.max_entries,
            "min_interval_ms": duration_millis(config.pacing.min_interval),
            "item_rate_per_second": config.pacing.item_rate_per_second,
            "deadline_ms": duration_millis(config.deadline),
            "cancel_after_ms": config.cancel_after.map(duration_millis),
        }),
    )?;

    let client =
        OpcDaClient::new(ComConnector).map_err(|error| OpcError::Internal(error.to_string()))?;
    let mut stream = client.start_inventory_at_path(
        &config.prog_id,
        config.options,
        config.start_path.clone(),
    )?;
    stream.set_pacing(config.pacing);

    let deadline_at = tokio::time::Instant::now() + config.deadline;
    let distant = tokio::time::Instant::now() + DISTANT_INVENTORY_TIMER;
    let cancel_at = config
        .cancel_after
        .map(|delay| tokio::time::Instant::now() + delay);
    let mut deadline_sleep = Box::pin(tokio::time::sleep_until(deadline_at));
    let mut cancel_sleep = Box::pin(tokio::time::sleep_until(cancel_at.unwrap_or(distant)));
    let mut grace_sleep = Box::pin(tokio::time::sleep_until(distant));
    let mut cancellation_requested = false;
    let mut deadline_expired = false;
    let mut terminal_event = false;
    let mut stream_error = None;
    let mut channel_closed = false;

    loop {
        tokio::select! {
            biased;
            message = stream.message() => {
                match message {
                    Some(Ok(event)) => match event {
                        InventoryEvent::Entry(entry) => {
                            write_inventory_json_line(
                                output,
                                "entry",
                                &serde_json::json!({
                                    "display_name": entry.display_name,
                                    "item_id": entry.item_id,
                                    "kind": browse_node_kind_name(entry.kind),
                                    "breadcrumbs": entry.breadcrumbs,
                                    "elapsed_ms": elapsed_millis(started_at),
                                }),
                            )?;
                        }
                        InventoryEvent::Progress(progress) => {
                            write_inventory_json_line(
                                output,
                                "progress",
                                &serde_json::json!({
                                    "branches_visited": progress.branches_visited,
                                    "entries_seen": progress.entries_seen,
                                    "unique_items": progress.unique_items,
                                    "active_time_ms": progress.active_time_ms,
                                    "paused_time_ms": progress.paused_time_ms,
                                    "items_per_second": progress.items_per_second,
                                    "estimated_remaining_ms": progress.estimated_remaining_ms,
                                    "elapsed_ms": elapsed_millis(started_at),
                                }),
                            )?;
                        }
                        InventoryEvent::Slice(slice) => {
                            write_inventory_json_line(
                                output,
                                "slice",
                                &serde_json::json!({
                                    "sequence": slice.sequence,
                                    "backend": format!("{:?}", slice.backend),
                                    "nodes_returned": slice.nodes_returned,
                                    "has_more": slice.has_more,
                                    "native_operations": slice.native_operations,
                                    "elapsed_ms": slice.elapsed_ms,
                                    "entries_seen": slice.entries_seen,
                                    "unique_items": slice.unique_items,
                                }),
                            )?;
                        }
                        InventoryEvent::Completed(completed) => {
                            write_inventory_completed(output, &completed, elapsed_millis(started_at))?;
                            terminal_event = true;
                            break;
                        }
                    },
                    Some(Err(error)) => {
                        let message = error.to_string();
                        write_inventory_json_line(
                            output,
                            "stream_error",
                            &serde_json::json!({
                                "error": message,
                                "elapsed_ms": elapsed_millis(started_at),
                            }),
                        )?;
                        stream_error = Some(message);
                        break;
                    }
                    None => {
                        write_inventory_json_line(
                            output,
                            "channel_closed",
                            &serde_json::json!({
                                "elapsed_ms": elapsed_millis(started_at),
                            }),
                        )?;
                        channel_closed = true;
                        break;
                    }
                }
            }
            () = &mut cancel_sleep, if !cancellation_requested && cancel_at.is_some() => {
                stream.cancel();
                cancellation_requested = true;
                write_inventory_json_line(
                    output,
                    "cancellation_requested",
                    &serde_json::json!({
                        "reason": "configured_delay",
                        "elapsed_ms": elapsed_millis(started_at),
                    }),
                )?;
            }
            () = &mut deadline_sleep, if !deadline_expired => {
                stream.cancel();
                cancellation_requested = true;
                deadline_expired = true;
                grace_sleep.as_mut().reset(tokio::time::Instant::now() + INVENTORY_CANCEL_GRACE);
                write_inventory_json_line(
                    output,
                    "deadline_expired",
                    &serde_json::json!({
                        "deadline_ms": duration_millis(config.deadline),
                        "elapsed_ms": elapsed_millis(started_at),
                    }),
                )?;
                write_inventory_json_line(
                    output,
                    "cancellation_requested",
                    &serde_json::json!({
                        "reason": "deadline",
                        "elapsed_ms": elapsed_millis(started_at),
                    }),
                )?;
            }
            () = &mut grace_sleep, if deadline_expired => {
                write_inventory_json_line(
                    output,
                    "cancellation_grace_expired",
                    &serde_json::json!({
                        "elapsed_ms": elapsed_millis(started_at),
                    }),
                )?;
                break;
            }
        }
    }

    let worker_join =
        if terminal_event || stream_error.is_some() || channel_closed || stream.worker_finished() {
            let result = stream.join_worker();
            write_inventory_worker_join(output, result, elapsed_millis(started_at))?;
            Some(result)
        } else {
            write_inventory_json_line(
                output,
                "worker_join",
                &serde_json::json!({
                    "status": "not_finished",
                    "elapsed_ms": elapsed_millis(started_at),
                }),
            )?;
            stream.detach_worker();
            None
        };

    let terminal = if terminal_event {
        InventoryTerminalState::Completed
    } else if stream_error.is_some() {
        InventoryTerminalState::StreamError
    } else if channel_closed {
        InventoryTerminalState::ChannelClosed
    } else {
        InventoryTerminalState::None
    };
    let (classification, lifecycle_ok) = inventory_lifecycle_result(InventoryLifecycleState {
        terminal,
        deadline_expired,
        worker_join,
    });
    write_inventory_json_line(
        output,
        "inventory_result",
        &serde_json::json!({
            "classification": classification,
            "terminal_event": terminal_event,
            "stream_error": stream_error,
            "channel_closed": channel_closed,
            "cancellation_requested": cancellation_requested,
            "deadline_expired": deadline_expired,
            "worker_joined": worker_join.is_some(),
            "lifecycle_ok": lifecycle_ok,
            "elapsed_ms": elapsed_millis(started_at),
        }),
    )?;

    if lifecycle_ok {
        Ok(())
    } else {
        Err(OpcError::Internal(format!(
            "native inventory lifecycle failed: {classification}"
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InventoryTerminalState {
    None,
    Completed,
    StreamError,
    ChannelClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InventoryLifecycleState {
    terminal: InventoryTerminalState,
    deadline_expired: bool,
    worker_join: Option<InventoryWorkerJoin>,
}

fn inventory_lifecycle_result(state: InventoryLifecycleState) -> (&'static str, bool) {
    let lifecycle_ok = matches!(state.terminal, InventoryTerminalState::Completed)
        && !state.deadline_expired
        && state.worker_join == Some(InventoryWorkerJoin::Returned);
    let classification = match state.terminal {
        InventoryTerminalState::StreamError => "stream_error",
        InventoryTerminalState::ChannelClosed => "channel_eof",
        _ if state.deadline_expired => "deadline",
        _ if lifecycle_ok => "completed",
        _ => "worker_failure",
    };
    (classification, lifecycle_ok)
}

fn write_inventory_completed(
    output: &mut impl Write,
    completed: &InventoryCompleted,
    elapsed_ms: u64,
) -> OpcResult<()> {
    write_inventory_json_line(
        output,
        "completed",
        &serde_json::json!({
            "complete": completed.complete,
            "cancelled": completed.cancelled,
            "truncated": completed.truncated,
            "warning": completed.warning,
            "namespace": format!("{:?}", completed.capabilities.namespace),
            "supports_da3": completed.capabilities.supports_da3,
            "supports_da2": completed.capabilities.supports_da2,
            "max_page_size": completed.capabilities.max_page_size,
            "elapsed_ms": elapsed_ms,
        }),
    )
}

fn write_inventory_worker_join(
    output: &mut impl Write,
    result: InventoryWorkerJoin,
    elapsed_ms: u64,
) -> OpcResult<()> {
    let (status, payload_type) = match result {
        InventoryWorkerJoin::Returned => ("returned", None),
        InventoryWorkerJoin::CaughtPanic { payload_type } => ("caught_panic", Some(payload_type)),
        InventoryWorkerJoin::UncaughtPanic => ("uncaught_panic", None),
    };
    write_inventory_json_line(
        output,
        "worker_join",
        &serde_json::json!({
            "status": status,
            "payload_type": payload_type,
            "elapsed_ms": elapsed_ms,
        }),
    )
}

fn browse_node_kind_name(kind: BrowseNodeKind) -> &'static str {
    match kind {
        BrowseNodeKind::Branch => "branch",
        BrowseNodeKind::Item => "item",
        BrowseNodeKind::BranchAndItem => "branch_and_item",
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_millis(started_at: Instant) -> u64 {
    duration_millis(started_at.elapsed())
}

fn write_inventory_json_line(
    output: &mut impl Write,
    event: &str,
    value: &impl Serialize,
) -> OpcResult<()> {
    write_json_line(output, event, value)
        .map_err(|error| OpcError::Internal(format!("failed to write inventory JSONL: {error}")))
}

/// Writes the report as newline-delimited JSON records.
pub fn write_json_lines(
    report: &NativeReadCanaryReport,
    mut output: impl Write,
) -> Result<(), serde_json::Error> {
    write_json_line(&mut output, "server", &ServerLine::from(report))?;
    write_json_line(&mut output, "status", &report.status)?;
    write_json_line(&mut output, "common", &report.common)?;
    for item in &report.items {
        write_json_line(&mut output, "item", item)?;
    }
    for property in &report.properties {
        write_json_line(&mut output, "property", property)?;
    }
    for property_error in &report.property_errors {
        write_json_line(&mut output, "property_error", property_error)?;
    }
    for read in &report.reads {
        write_json_line(&mut output, "read", read)?;
    }
    write_json_line(&mut output, "cleanup", &report.cleanup)?;
    if let Some(comparison) = &report.worker_comparison {
        write_json_line(&mut output, "worker_comparison", comparison)?;
    }
    write_json_line(
        &mut output,
        "complete",
        &CompleteLine {
            direct_error: report.direct_error.as_deref(),
        },
    )
}

fn validate_config(config: &NativeReadCanaryConfig) -> OpcResult<()> {
    if config.prog_id.trim().is_empty() {
        return Err(OpcError::InvalidState(
            "diagnostic ProgID cannot be empty".to_string(),
        ));
    }
    if config.item_ids.is_empty() || config.item_ids.iter().any(|item| item.trim().is_empty()) {
        return Err(OpcError::InvalidState(
            "diagnostic ItemIDs cannot be empty".to_string(),
        ));
    }
    if !(MIN_UPDATE_RATE_MS..=MAX_UPDATE_RATE_MS).contains(&config.requested_update_rate_ms) {
        return Err(OpcError::InvalidState(format!(
            "diagnostic update rate must be between {MIN_UPDATE_RATE_MS} and {MAX_UPDATE_RATE_MS} ms"
        )));
    }
    if config.deadline.is_zero() || config.deadline > MAX_DEADLINE {
        return Err(OpcError::InvalidState(format!(
            "diagnostic deadline must be between 1 ms and {MAX_DEADLINE:?}"
        )));
    }
    if let Some(item) = &config.worker_compare_item
        && !config.item_ids.contains(item)
    {
        return Err(OpcError::InvalidState(
            "worker comparison ItemID must be included in item_ids".to_string(),
        ));
    }
    Ok(())
}

fn run_direct_canary<C: ServerConnector>(
    connector: &C,
    config: &NativeReadCanaryConfig,
) -> OpcResult<NativeReadCanaryReport> {
    let deadline = Instant::now()
        .checked_add(config.deadline)
        .ok_or_else(|| OpcError::InvalidState("diagnostic deadline overflowed".to_string()))?;
    let server = connector.connect(&config.prog_id)?;
    let status = Captured::from_result(server.diagnostic_status().map(server_status_snapshot));
    let locale_id = Captured::from_result(server.diagnostic_locale_id());
    let available_locale_ids = Captured::from_result(server.diagnostic_available_locale_ids());
    let yokogawa_error_probe = describe_hresult(&server, YOKOGAWA_PROBE_HRESULT);

    let mut revised_update_rate_ms = 0;
    let mut server_handle = GroupHandle::default();
    let group = server.add_group(
        &format!("bytehound-native-canary-{}", uuid::Uuid::new_v4()),
        true,
        config.requested_update_rate_ms,
        GroupHandle(1),
        0,
        0.0,
        locale_id.value.unwrap_or(0),
        &mut revised_update_rate_ms,
        &mut server_handle,
    )?;
    if !(MIN_UPDATE_RATE_MS..=MAX_UPDATE_RATE_MS).contains(&revised_update_rate_ms) {
        let rate_error = format!(
            "server revised update rate to {revised_update_rate_ms} ms, outside the supported range \
             {MIN_UPDATE_RATE_MS}..={MAX_UPDATE_RATE_MS} ms"
        );
        return match server.remove_group(server_handle, true) {
            Ok(()) => Err(OpcError::Internal(rate_error)),
            Err(cleanup_error) => Err(OpcError::Internal(format!(
                "{rate_error}; group cleanup also failed: {cleanup_error}"
            ))),
        };
    }

    let mut report = NativeReadCanaryReport {
        prog_id: config.prog_id.clone(),
        requested_update_rate_ms: config.requested_update_rate_ms,
        revised_update_rate_ms,
        status,
        common: ServerCommonSnapshot {
            locale_id,
            available_locale_ids,
            yokogawa_error_probe,
        },
        items: Vec::new(),
        properties: Vec::new(),
        property_errors: Vec::new(),
        reads: Vec::new(),
        cleanup: Captured {
            value: None,
            error: None,
        },
        direct_error: None,
        worker_comparison: None,
    };

    let body_result = run_group_canary(&server, &group, config, &mut report, deadline);
    report.cleanup = Captured::from_result(server.remove_group(server_handle, true).map(|()| true));
    if let Err(error) = body_result {
        report.direct_error = Some(error.to_string());
    }
    Ok(report)
}

fn run_group_canary<S: ConnectedServer>(
    server: &S,
    group: &S::Group,
    config: &NativeReadCanaryConfig,
    report: &mut NativeReadCanaryReport,
    deadline: Instant,
) -> OpcResult<()> {
    let item_id_wides = config
        .item_ids
        .iter()
        .map(|item_id| item_id.encode_utf16().chain(std::iter::once(0)).collect())
        .collect::<Vec<Vec<u16>>>();
    let item_defs = build_item_defs(&item_id_wides);
    let (validation_results, validation_errors) = group.validate_items(&item_defs)?;
    require_lengths(
        "ValidateItems",
        config.item_ids.len(),
        validation_results.len(),
        validation_errors.len(),
    )?;
    let (add_results, add_errors) = group.add_items(&item_defs)?;
    require_lengths(
        "AddItems",
        config.item_ids.len(),
        add_results.len(),
        add_errors.len(),
    )?;

    let mut added_items = Vec::new();
    for index in 0..config.item_ids.len() {
        let validation_error = validation_errors.as_slice()[index];
        let add_error = add_errors.as_slice()[index];
        let add_result = &add_results.as_slice()[index];
        report.items.push(ItemRegistration {
            item_id: config.item_ids[index].clone(),
            validation: describe_hresult(server, validation_error),
            validation_metadata: if validation_error.is_ok() {
                Some(item_metadata(&validation_results.as_slice()[index])?)
            } else {
                None
            },
            addition: describe_hresult(server, add_error),
            added_metadata: if add_error.is_ok() {
                Some(item_metadata(add_result)?)
            } else {
                None
            },
        });
        if add_error.is_ok() {
            added_items.push((
                config.item_ids[index].clone(),
                ItemHandle(add_result.hServer),
            ));
        }
    }

    for item_id in &config.item_ids {
        match server.diagnostic_item_properties(item_id) {
            Ok(properties) => report.properties.extend(
                properties
                    .into_iter()
                    .map(|property| property_snapshot(server, item_id, property)),
            ),
            Err(error) => report.property_errors.push(Captured {
                value: Some(item_id.clone()),
                error: Some(error.to_string()),
            }),
        }
    }

    if added_items.is_empty() {
        return Ok(());
    }

    ensure_before_deadline(deadline)?;
    capture_read_pair(server, group, ReadPhase::Immediate, &added_items, report)?;
    wait_for_interval(report.revised_update_rate_ms, deadline)?;
    capture_read_pair(
        server,
        group,
        ReadPhase::AfterOneInterval,
        &added_items,
        report,
    )?;
    wait_for_interval(report.revised_update_rate_ms, deadline)?;
    capture_read_pair(
        server,
        group,
        ReadPhase::AfterTwoIntervals,
        &added_items,
        report,
    )
}

fn ensure_before_deadline(deadline: Instant) -> OpcResult<()> {
    if Instant::now() >= deadline {
        return Err(OpcError::Internal(
            "native diagnostic deadline expired before the next operation".to_string(),
        ));
    }
    Ok(())
}

fn wait_for_interval(interval_ms: u32, deadline: Instant) -> OpcResult<()> {
    let interval = Duration::from_millis(u64::from(interval_ms));
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < interval {
        std::thread::sleep(remaining);
        return Err(OpcError::Internal(
            "native diagnostic deadline expired during interval wait".to_string(),
        ));
    }
    std::thread::sleep(interval);
    Ok(())
}

fn capture_read_pair<S: ConnectedServer>(
    server: &S,
    group: &S::Group,
    phase: ReadPhase,
    added_items: &[(String, ItemHandle)],
    report: &mut NativeReadCanaryReport,
) -> OpcResult<()> {
    for source in [ReadSource::Device, ReadSource::Cache] {
        let handles = added_items
            .iter()
            .map(|(_, handle)| *handle)
            .collect::<Vec<_>>();
        match group.read(source.native(), &handles) {
            Ok((states, errors)) => report.reads.push(map_read_batch(
                server,
                phase,
                source,
                added_items,
                &states,
                &errors,
            )?),
            Err(error) => report.reads.push(DirectReadBatch {
                phase,
                source,
                items: Vec::new(),
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(())
}

fn map_read_batch<S: ConnectedServer>(
    server: &S,
    phase: ReadPhase,
    source: ReadSource,
    items: &[(String, ItemHandle)],
    states: &RemoteArray<tagOPCITEMSTATE>,
    errors: &RemoteArray<HRESULT>,
) -> OpcResult<DirectReadBatch> {
    require_lengths("Read", items.len(), states.len(), errors.len())?;
    let mapped = items
        .iter()
        .enumerate()
        .map(|(index, (item_id, _))| {
            let state = &states.as_slice()[index];
            let error = errors.as_slice()[index];
            DirectReadItem {
                item_id: item_id.clone(),
                result: describe_hresult(server, error),
                value: error.is_ok().then(|| variant_to_string(&state.vDataValue)),
                quality: error.is_ok().then(|| quality_to_string(state.wQuality)),
                raw_quality: error.is_ok().then_some(state.wQuality),
                timestamp: error.is_ok().then(|| filetime_to_string(state.ftTimeStamp)),
            }
        })
        .collect();
    Ok(DirectReadBatch {
        phase,
        source,
        items: mapped,
        error: None,
    })
}

fn require_lengths(operation: &str, expected: usize, first: u32, second: u32) -> OpcResult<()> {
    let first = usize::try_from(first)?;
    let second = usize::try_from(second)?;
    if first != expected || second != expected {
        return Err(OpcError::Internal(format!(
            "{operation} returned malformed result lengths: expected {expected}, got {first} and {second}"
        )));
    }
    Ok(())
}

fn describe_hresult<S: ConnectedServer>(server: &S, error: HRESULT) -> HResultSnapshot {
    describe_hresult_with(error, |code| server.diagnostic_error_string(code))
}

fn describe_hresult_with<E: std::fmt::Display>(
    error: HRESULT,
    vendor_lookup: impl FnOnce(HRESULT) -> Result<String, E>,
) -> HResultSnapshot {
    let windows_text = windows::core::Error::from(error).message();
    let vendor = vendor_lookup(error);
    let (vendor_text, vendor_lookup_error) = match vendor {
        Ok(text) => (Some(text), None),
        Err(error) => (None, Some(error.to_string())),
    };
    HResultSnapshot {
        code: error.0,
        hex: format!("0x{:08X}", error.0.cast_unsigned()),
        succeeded: error.is_ok(),
        normal_text: if windows_text.trim().is_empty() {
            crate::format_hresult(error)
        } else {
            windows_text
        },
        vendor_text,
        vendor_lookup_error,
    }
}

fn server_status_snapshot(status: DiagnosticServerStatus) -> ServerStatusSnapshot {
    ServerStatusSnapshot {
        start_time: system_time_to_rfc3339(status.start_time),
        current_time: system_time_to_rfc3339(status.current_time),
        last_update_time: system_time_to_rfc3339(status.last_update_time),
        state: status.server_state,
        group_count: status.group_count,
        bandwidth: status.band_width,
        version: format!(
            "{}.{}.{}",
            status.major_version, status.minor_version, status.build_number
        ),
        vendor_info: status.vendor_info,
    }
}

fn system_time_to_rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

fn item_metadata(result: &tagOPCITEMRESULT) -> OpcResult<ItemMetadata> {
    use crate::opc_da::com_utils::TryFromNative;
    use crate::opc_da::typedefs::ItemResult;

    let result = ItemResult::try_from_native(result)?;
    Ok(ItemMetadata {
        canonical_data_type: result.data_type,
        access_rights: result.access_rights,
        server_handle: result.server_handle.0,
    })
}

fn property_snapshot<S: ConnectedServer>(
    server: &S,
    item_id: &str,
    property: DiagnosticItemProperty,
) -> ItemPropertySnapshot {
    ItemPropertySnapshot {
        item_id: item_id.to_string(),
        property_id: property.id,
        description: property.description,
        data_type: property.data_type,
        value: property.value,
        result: describe_hresult(server, property.error),
    }
}

fn build_item_defs(item_id_wides: &[Vec<u16>]) -> Vec<tagOPCITEMDEF> {
    item_id_wides
        .iter()
        .enumerate()
        .map(|(index, item_id)| tagOPCITEMDEF {
            szAccessPath: windows::core::PWSTR::null(),
            szItemID: windows::core::PWSTR(item_id.as_ptr().cast_mut()),
            bActive: windows::Win32::Foundation::TRUE,
            #[allow(clippy::cast_possible_truncation)]
            hClient: index as u32,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
            vtRequestedDataType: 0,
            wReserved: 0,
        })
        .collect()
}

fn worker_value(value: TagValue) -> WorkerReadValue {
    WorkerReadValue {
        item_id: value.tag_id,
        value: value.value,
        quality: value.quality,
        timestamp: value.timestamp,
    }
}

fn compare_worker_read(
    report: &NativeReadCanaryReport,
    item_id: &str,
    worker: Captured<WorkerReadValue>,
) -> WorkerComparison {
    let direct = report
        .reads
        .iter()
        .find(|read| read.phase == ReadPhase::Immediate && read.source == ReadSource::Device)
        .and_then(|read| read.items.iter().find(|item| item.item_id == item_id))
        .and_then(|item| {
            Some(WorkerReadValue {
                item_id: item.item_id.clone(),
                value: item.value.clone()?,
                quality: item.quality.clone()?,
                timestamp: item.timestamp.clone()?,
            })
        });
    let worker_value = worker.value.as_ref();
    WorkerComparison {
        item_id: item_id.to_string(),
        same_value: direct
            .as_ref()
            .zip(worker_value)
            .map(|(direct, worker)| direct.value == worker.value),
        same_quality: direct
            .as_ref()
            .zip(worker_value)
            .map(|(direct, worker)| direct.quality == worker.quality),
        same_timestamp: direct
            .as_ref()
            .zip(worker_value)
            .map(|(direct, worker)| direct.timestamp == worker.timestamp),
        direct,
        worker,
    }
}

#[derive(Serialize)]
struct JsonLine<'a, T> {
    event: &'a str,
    data: &'a T,
}

fn write_json_line<T: Serialize>(
    output: &mut impl Write,
    event: &str,
    data: &T,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut *output, &JsonLine { event, data })?;
    output.write_all(b"\n").map_err(serde_json::Error::io)
}

#[derive(Serialize)]
struct ServerLine<'a> {
    prog_id: &'a str,
    requested_update_rate_ms: u32,
    revised_update_rate_ms: u32,
}

impl<'a> From<&'a NativeReadCanaryReport> for ServerLine<'a> {
    fn from(report: &'a NativeReadCanaryReport) -> Self {
        Self {
            prog_id: &report.prog_id,
            requested_update_rate_ms: report.requested_update_rate_ms,
            revised_update_rate_ms: report.revised_update_rate_ms,
        }
    }
}

#[derive(Serialize)]
struct CompleteLine<'a> {
    direct_error: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::connector::{
        BrowseStringIterator, DiagnosticItemProperty, NativeBrowsePage, StringIterator,
    };
    use crate::provider::BrowseNodeFilter;
    use std::sync::{Arc, Mutex};
    use windows::Win32::Foundation::{E_FAIL, FILETIME, S_OK};

    fn remote_array<T>(values: Vec<T>) -> RemoteArray<T> {
        if values.is_empty() {
            return RemoteArray::empty();
        }
        let len = u32::try_from(values.len()).unwrap();
        let size = std::mem::size_of::<T>() * values.len();
        // SAFETY: CoTaskMemAlloc returns storage suitable for a COM-owned array.
        let pointer = unsafe { windows::Win32::System::Com::CoTaskMemAlloc(size) }.cast::<T>();
        assert!(!pointer.is_null());
        for (index, value) in values.into_iter().enumerate() {
            // SAFETY: The allocation contains exactly `len` elements.
            unsafe { pointer.add(index).write(value) };
        }
        // SAFETY: pointer is a freshly allocated COM task buffer containing
        // exactly len initialized values, and ownership transfers here.
        unsafe { RemoteArray::from_mut_ptr(pointer, len) }
    }

    #[derive(Clone)]
    struct MockConnector {
        state: Arc<MockState>,
    }

    struct MockState {
        validate_count: usize,
        add_count: usize,
        read_count: usize,
        failed_source: Mutex<Option<ReadSource>>,
        sources: Mutex<Vec<ReadSource>>,
        removed: Mutex<Vec<GroupHandle>>,
        vendor_codes: Mutex<Vec<HRESULT>>,
    }

    struct MockServer {
        state: Arc<MockState>,
    }

    struct MockGroup {
        state: Arc<MockState>,
    }

    impl ServerConnector for MockConnector {
        type Server = MockServer;

        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(vec!["Mock.Server".to_string()])
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(MockServer {
                state: Arc::clone(&self.state),
            })
        }
    }

    impl ConnectedServer for MockServer {
        type Group = MockGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(0)
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn begin_da2_browse(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn browse_da3(
            &self,
            _item_id: Option<&str>,
            _continuation: Option<&str>,
            _max_elements: u32,
            _filter: BrowseNodeFilter,
        ) -> OpcResult<NativeBrowsePage> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            revised_update_rate: &mut u32,
            server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            *revised_update_rate = update_rate;
            *server_handle = GroupHandle(42);
            Ok(MockGroup {
                state: Arc::clone(&self.state),
            })
        }

        fn remove_group(&self, server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            self.state.removed.lock().unwrap().push(server_group);
            Ok(())
        }

        fn diagnostic_status(&self) -> OpcResult<DiagnosticServerStatus> {
            Ok(DiagnosticServerStatus {
                start_time: SystemTime::UNIX_EPOCH,
                current_time: SystemTime::UNIX_EPOCH,
                last_update_time: SystemTime::UNIX_EPOCH,
                server_state: "running".to_string(),
                group_count: 1,
                band_width: 2,
                major_version: 3,
                minor_version: 4,
                build_number: 5,
                vendor_info: "Mock".to_string(),
            })
        }

        fn diagnostic_locale_id(&self) -> OpcResult<u32> {
            Ok(1_033)
        }

        fn diagnostic_available_locale_ids(&self) -> OpcResult<Vec<u32>> {
            Ok(vec![1_033, 0])
        }

        fn diagnostic_error_string(&self, error: HRESULT) -> OpcResult<String> {
            self.state.vendor_codes.lock().unwrap().push(error);
            Ok(format!("vendor 0x{:08X}", error.0.cast_unsigned()))
        }

        fn diagnostic_item_properties(
            &self,
            item_id: &str,
        ) -> OpcResult<Vec<DiagnosticItemProperty>> {
            Ok(vec![DiagnosticItemProperty {
                id: 101,
                description: format!("description for {item_id}"),
                data_type: 8,
                value: Some("value".to_string()),
                error: S_OK,
            }])
        }
    }

    impl ConnectedGroup for MockGroup {
        fn validate_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(RemoteArray<tagOPCITEMRESULT>, RemoteArray<HRESULT>)> {
            Ok((
                remote_array(
                    (0..self.state.validate_count)
                        .map(|index| item_result(u32::try_from(index).unwrap() + 1))
                        .collect(),
                ),
                remote_array(vec![S_OK; self.state.validate_count]),
            ))
        }

        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(RemoteArray<tagOPCITEMRESULT>, RemoteArray<HRESULT>)> {
            Ok((
                remote_array(
                    (0..self.state.add_count)
                        .map(|index| item_result(u32::try_from(index).unwrap() + 10))
                        .collect(),
                ),
                remote_array(vec![S_OK; self.state.add_count]),
            ))
        }

        fn read(
            &self,
            source: tagOPCDATASOURCE,
            _server_handles: &[ItemHandle],
        ) -> OpcResult<(RemoteArray<tagOPCITEMSTATE>, RemoteArray<HRESULT>)> {
            let source = if source == OPC_DS_DEVICE {
                ReadSource::Device
            } else {
                ReadSource::Cache
            };
            self.state.sources.lock().unwrap().push(source);
            if self.state.failed_source.lock().unwrap().as_ref() == Some(&source) {
                return Err(OpcError::Internal(format!("{source:?} read failed")));
            }
            Ok((
                remote_array(
                    (0..self.state.read_count)
                        .map(|index| item_state(f64::from(u32::try_from(index).unwrap()) + 1.0))
                        .collect(),
                ),
                remote_array(vec![S_OK; self.state.read_count]),
            ))
        }

        fn write(
            &self,
            _server_handles: &[ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<HRESULT>> {
            Err(OpcError::InvalidState(
                "diagnostic mock is read-only".to_string(),
            ))
        }
    }

    fn item_result(handle: u32) -> tagOPCITEMRESULT {
        tagOPCITEMRESULT {
            hServer: handle,
            vtCanonicalDataType: 5,
            wReserved: 0,
            dwAccessRights: 1,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
        }
    }

    fn item_state(value: f64) -> tagOPCITEMSTATE {
        tagOPCITEMSTATE {
            hClient: 0,
            ftTimeStamp: FILETIME::default(),
            wQuality: 0xC0,
            wReserved: 0,
            vDataValue: crate::helpers::opc_value_to_variant(&crate::provider::OpcValue::Float(
                value,
            )),
        }
    }

    fn mock_connector(validate_count: usize, add_count: usize, read_count: usize) -> MockConnector {
        MockConnector {
            state: Arc::new(MockState {
                validate_count,
                add_count,
                read_count,
                failed_source: Mutex::new(None),
                sources: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                vendor_codes: Mutex::new(Vec::new()),
            }),
        }
    }

    #[test]
    fn malformed_validation_lengths_are_reported_and_group_is_cleaned_up() {
        let connector = mock_connector(0, 1, 1);
        let config = NativeReadCanaryConfig {
            prog_id: "Mock.Server".to_string(),
            item_ids: vec!["Tag".to_string()],
            requested_update_rate_ms: 1,
            deadline: DEFAULT_DEADLINE,
            worker_compare_item: None,
        };
        let report = run_direct_canary(&connector, &config).unwrap();
        assert!(
            report
                .direct_error
                .as_deref()
                .is_some_and(|error| error.contains("ValidateItems"))
        );
        assert_eq!(*connector.state.removed.lock().unwrap(), [GroupHandle(42)]);
        assert_eq!(report.cleanup.value, Some(true));
    }

    #[test]
    fn malformed_add_lengths_are_reported_and_group_is_cleaned_up() {
        let connector = mock_connector(1, 0, 1);
        let config = NativeReadCanaryConfig::new("Mock.Server", vec!["Tag".to_string()]);
        let report = run_direct_canary(&connector, &config).unwrap();
        assert!(
            report
                .direct_error
                .as_deref()
                .is_some_and(|error| error.contains("AddItems"))
        );
        assert_eq!(*connector.state.removed.lock().unwrap(), [GroupHandle(42)]);
    }

    #[test]
    fn malformed_read_lengths_are_reported_and_group_is_cleaned_up() {
        let connector = mock_connector(1, 1, 0);
        let config = NativeReadCanaryConfig::new("Mock.Server", vec!["Tag".to_string()]);
        let report = run_direct_canary(&connector, &config).unwrap();
        assert!(
            report
                .direct_error
                .as_deref()
                .is_some_and(|error| error.contains("Read"))
        );
        assert_eq!(*connector.state.removed.lock().unwrap(), [GroupHandle(42)]);
    }

    #[test]
    fn device_and_cache_sources_are_both_explicit_and_never_substituted() {
        let connector = mock_connector(1, 1, 1);
        let mut config = NativeReadCanaryConfig::new("Mock.Server", vec!["Tag".to_string()]);
        config.requested_update_rate_ms = 1;
        let report = run_direct_canary(&connector, &config).unwrap();
        assert_eq!(report.direct_error, None);
        assert_eq!(
            *connector.state.sources.lock().unwrap(),
            [
                ReadSource::Device,
                ReadSource::Cache,
                ReadSource::Device,
                ReadSource::Cache,
                ReadSource::Device,
                ReadSource::Cache,
            ]
        );
    }

    #[test]
    fn failed_device_reads_do_not_prevent_independent_cache_reads() {
        let connector = mock_connector(1, 1, 1);
        *connector.state.failed_source.lock().unwrap() = Some(ReadSource::Device);
        let mut config = NativeReadCanaryConfig::new("Mock.Server", vec!["Tag".to_string()]);
        config.requested_update_rate_ms = 1;
        let report = run_direct_canary(&connector, &config).unwrap();
        assert_eq!(report.direct_error, None);
        assert_eq!(report.reads.len(), 6);
        assert!(
            report
                .reads
                .iter()
                .filter(|read| read.source == ReadSource::Device)
                .all(|read| read.error.is_some())
        );
        assert!(
            report
                .reads
                .iter()
                .filter(|read| read.source == ReadSource::Cache)
                .all(|read| read.error.is_none())
        );
    }

    #[test]
    fn hresult_mapping_captures_normal_and_vendor_text() {
        let result = describe_hresult_with(YOKOGAWA_PROBE_HRESULT, |error| {
            Ok::<_, OpcError>(format!("vendor {}", error.0))
        });
        assert_eq!(result.hex, "0xC004800B");
        assert!(!result.succeeded);
        assert!(!result.normal_text.is_empty());
        assert_eq!(result.vendor_text.as_deref(), Some("vendor -1073446901"));
        assert_eq!(result.vendor_lookup_error, None);

        let failed = describe_hresult_with(E_FAIL, |_| {
            Err::<String, _>(OpcError::NotImplemented("lookup".to_string()))
        });
        assert!(failed.vendor_text.is_none());
        assert!(
            failed
                .vendor_lookup_error
                .as_deref()
                .is_some_and(|error| error.contains("lookup"))
        );
    }

    #[test]
    fn failed_per_item_read_preserves_hresult_and_omits_value_fields() {
        let connector = mock_connector(1, 1, 1);
        let server = connector.connect("Mock.Server").unwrap();
        let states = remote_array(vec![item_state(42.0)]);
        let errors = remote_array(vec![E_FAIL]);
        let batch = map_read_batch(
            &server,
            ReadPhase::Immediate,
            ReadSource::Device,
            &[("Tag".to_string(), ItemHandle(10))],
            &states,
            &errors,
        )
        .unwrap();
        assert_eq!(batch.items[0].result.hex, "0x80004005");
        assert_eq!(batch.items[0].value, None);
        assert_eq!(batch.items[0].quality, None);
        assert_eq!(batch.items[0].raw_quality, None);
        assert_eq!(batch.items[0].timestamp, None);
        assert_eq!(
            batch.items[0].result.vendor_text.as_deref(),
            Some("vendor 0x80004005")
        );
    }

    #[test]
    fn direct_run_plumbs_all_hresult_values_through_vendor_lookup() {
        let connector = mock_connector(1, 1, 1);
        let mut config = NativeReadCanaryConfig::new("Mock.Server", vec!["Tag".to_string()]);
        config.requested_update_rate_ms = 1;
        let report = run_direct_canary(&connector, &config).unwrap();
        assert_eq!(report.direct_error, None);
        let codes = connector.state.vendor_codes.lock().unwrap();
        assert!(codes.contains(&YOKOGAWA_PROBE_HRESULT));
        assert!(codes.iter().any(|code| code.is_ok()));
        drop(codes);
    }

    #[test]
    fn comparison_uses_immediate_device_mapping() {
        let report = NativeReadCanaryReport {
            prog_id: "Mock".to_string(),
            requested_update_rate_ms: 1,
            revised_update_rate_ms: 1,
            status: Captured {
                value: None,
                error: None,
            },
            common: ServerCommonSnapshot {
                locale_id: Captured {
                    value: None,
                    error: None,
                },
                available_locale_ids: Captured {
                    value: None,
                    error: None,
                },
                yokogawa_error_probe: describe_hresult_with(S_OK, |_| {
                    Ok::<_, OpcError>("ok".to_string())
                }),
            },
            items: Vec::new(),
            properties: Vec::new(),
            property_errors: Vec::new(),
            reads: vec![DirectReadBatch {
                phase: ReadPhase::Immediate,
                source: ReadSource::Device,
                items: vec![DirectReadItem {
                    item_id: "Tag".to_string(),
                    result: describe_hresult_with(S_OK, |_| Ok::<_, OpcError>("ok".to_string())),
                    value: Some("1.00".to_string()),
                    quality: Some("Good".to_string()),
                    raw_quality: Some(0xC0),
                    timestamp: Some("N/A".to_string()),
                }],
                error: None,
            }],
            cleanup: Captured {
                value: Some(true),
                error: None,
            },
            direct_error: None,
            worker_comparison: None,
        };
        let comparison = compare_worker_read(
            &report,
            "Tag",
            Captured {
                value: Some(WorkerReadValue {
                    item_id: "Tag".to_string(),
                    value: "1.00".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "N/A".to_string(),
                }),
                error: None,
            },
        );
        assert_eq!(comparison.same_value, Some(true));
        assert_eq!(comparison.same_quality, Some(true));
        assert_eq!(comparison.same_timestamp, Some(true));
    }

    #[test]
    fn json_lines_escape_server_and_item_text() {
        let connector = mock_connector(1, 1, 1);
        let mut config =
            NativeReadCanaryConfig::new("Mock.\"Server", vec!["Area\\Tag\nOne".to_string()]);
        config.requested_update_rate_ms = 1;
        let report = run_direct_canary(&connector, &config).unwrap();
        let mut output = Vec::new();
        write_json_lines(&report, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains(r#"Mock.\"Server"#));
        assert!(text.contains(r"Area\\Tag\nOne"));
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
    }

    #[test]
    fn config_rejects_invalid_worker_comparison_item() {
        let config = NativeReadCanaryConfig {
            prog_id: "Mock".to_string(),
            item_ids: vec!["Tag".to_string()],
            requested_update_rate_ms: 1,
            deadline: DEFAULT_DEADLINE,
            worker_compare_item: Some("Other".to_string()),
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn inventory_config_rejects_invalid_bounds_before_native_work() {
        let mut config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.options.batch_size = 0;
        assert!(config.validate().is_err());

        config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.deadline = Duration::ZERO;
        assert!(config.validate().is_err());

        config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.cancel_after = Some(config.deadline);
        assert!(config.validate().is_err());

        config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.start_path = Some(vec!["FCS0219".to_string(), String::new()]);
        assert!(config.validate().is_err());

        config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.start_path = Some(vec!["-not-a-branch".to_string()]);
        assert!(config.validate().is_err());

        config = NativeInventoryCanaryConfig::new("Mock.Server");
        config.start_path = Some(vec!["FCS\0".to_string()]);
        assert!(config.validate().is_err());
    }

    #[test]
    fn inventory_lifecycle_joins_after_terminal_event_even_if_finished_flag_was_not_sampled() {
        assert_eq!(
            inventory_lifecycle_result(InventoryLifecycleState {
                terminal: InventoryTerminalState::Completed,
                deadline_expired: false,
                worker_join: Some(InventoryWorkerJoin::Returned),
            },),
            ("completed", true)
        );
    }

    #[test]
    fn inventory_lifecycle_classifies_each_non_success_terminal_shape() {
        assert_eq!(
            inventory_lifecycle_result(InventoryLifecycleState {
                terminal: InventoryTerminalState::StreamError,
                deadline_expired: false,
                worker_join: None,
            }),
            ("stream_error", false)
        );
        assert_eq!(
            inventory_lifecycle_result(InventoryLifecycleState {
                terminal: InventoryTerminalState::ChannelClosed,
                deadline_expired: false,
                worker_join: None,
            }),
            ("channel_eof", false)
        );
        assert_eq!(
            inventory_lifecycle_result(InventoryLifecycleState {
                terminal: InventoryTerminalState::None,
                deadline_expired: true,
                worker_join: None,
            }),
            ("deadline", false)
        );
        assert_eq!(
            inventory_lifecycle_result(InventoryLifecycleState {
                terminal: InventoryTerminalState::Completed,
                deadline_expired: false,
                worker_join: Some(InventoryWorkerJoin::UncaughtPanic),
            }),
            ("worker_failure", false)
        );
    }

    #[test]
    fn inventory_json_lines_escape_control_characters() {
        let mut output = Vec::new();
        write_inventory_json_line(
            &mut output,
            "entry",
            &serde_json::json!({
                "display_name": "bad\u{0001}name",
                "item_id": "Area\u{0000}Tag",
                "breadcrumbs": ["Root\u{000B}Branch"],
            }),
        )
        .unwrap();

        let line = String::from_utf8(output).unwrap();
        assert!(line.contains(r"\u0001"));
        assert!(line.contains(r"\u0000"));
        assert!(line.contains(r"\u000b"));
        serde_json::from_str::<serde_json::Value>(line.trim()).unwrap();
    }

    #[test]
    fn inventory_worker_join_serializes_all_worker_states() {
        for (result, expected) in [
            (InventoryWorkerJoin::Returned, r#""status":"returned""#),
            (
                InventoryWorkerJoin::CaughtPanic {
                    payload_type: "String",
                },
                r#""status":"caught_panic""#,
            ),
            (
                InventoryWorkerJoin::UncaughtPanic,
                r#""status":"uncaught_panic""#,
            ),
        ] {
            let mut output = Vec::new();
            write_inventory_worker_join(&mut output, result, 7).unwrap();
            let line = String::from_utf8(output).unwrap();
            assert!(line.contains(expected));
            serde_json::from_str::<serde_json::Value>(line.trim()).unwrap();
        }
    }
}
