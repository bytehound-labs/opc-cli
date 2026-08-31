//! Bounded namespace inventory traversal used by the bridge search index.

use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, Da2BranchNavigation, NativeBrowseElement,
    ServerConnector, guard_browse_iterator,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT,
};
use crate::opc_da::errors::{
    E_INVALIDARG_HRESULT, OpcError, OpcResult, com_hresult, contextual_browse_error,
    is_da3_browse_compatibility_error, is_non_progress_browse_error,
};
use crate::provider::{
    BrowseCapabilities, BrowseNodeFilter, BrowseNodeKind, InventoryCompleted, InventoryControl,
    InventoryEntry, InventoryEvent, InventoryOptions, InventoryProgress, InventorySliceBackend,
    InventorySliceObservation,
};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

struct BranchWork {
    location: BranchLocation,
    breadcrumbs: Vec<String>,
    da3_continuation: Option<String>,
    da2_state: Option<Da2PageState>,
}

enum BranchLocation {
    Da3(Option<String>),
    Da2(Vec<String>),
}

struct InventoryNode {
    display_name: String,
    item_id: Option<String>,
    kind: BrowseNodeKind,
    child: Option<BranchLocation>,
}

struct InventoryPage {
    nodes: Vec<InventoryNode>,
    continuation: Option<InventoryContinuation>,
}

enum InventoryContinuation {
    Da3(String),
    Da2(Box<Da2PageState>),
}

struct InventoryDa2BranchNode {
    kind: BrowseNodeKind,
    item_id: Option<String>,
    child: Option<BranchLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryResult {
    Proceed,
    Cancelled,
}

#[derive(Debug)]
enum InventoryError {
    Cancelled,
    Failed(OpcError),
}

impl From<OpcError> for InventoryError {
    fn from(error: OpcError) -> Self {
        Self::Failed(error)
    }
}

fn contextual_inventory_error(
    error: InventoryError,
    operation: &str,
    path: &[String],
    item: Option<&str>,
) -> InventoryError {
    match error {
        InventoryError::Failed(error) => {
            InventoryError::Failed(contextual_browse_error(error, operation, path, item))
        }
        InventoryError::Cancelled => InventoryError::Cancelled,
    }
}

/// Gate every bounded native operation on pause/cancellation and current pacing.
struct InventoryBoundary<'a> {
    control: &'a InventoryControl,
    last_started: Option<Instant>,
    paused_time: Duration,
    native_operations: u64,
    first_operation_reported: bool,
}

fn pacing_interval(pacing: crate::provider::InventoryPacing, item_cost: u32) -> Duration {
    let item_interval = pacing
        .item_rate_per_second
        .filter(|rate| *rate > 0)
        .map_or(Duration::ZERO, |rate| {
            Duration::from_secs_f64(f64::from(item_cost.max(1)) / f64::from(rate))
        });
    pacing.min_interval.max(item_interval)
}

impl<'a> InventoryBoundary<'a> {
    fn new(control: &'a InventoryControl) -> Self {
        Self {
            control,
            last_started: None,
            paused_time: Duration::ZERO,
            native_operations: 0,
            first_operation_reported: false,
        }
    }

    fn before_operation(&mut self) -> BoundaryResult {
        self.before_operation_with_cost(1)
    }

    fn before_operation_with_cost(&mut self, item_cost: u32) -> BoundaryResult {
        loop {
            if self.control.is_cancelled() {
                return BoundaryResult::Cancelled;
            }

            if self.control.is_paused() {
                let started = Instant::now();
                while self.control.is_paused() && !self.control.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.paused_time += started.elapsed();
                continue;
            }

            let interval = pacing_interval(self.control.pacing(), item_cost);
            if let Some(last_started) = self.last_started {
                let elapsed = last_started.elapsed();
                if let Some(remaining) = interval.checked_sub(elapsed) {
                    std::thread::sleep(remaining.min(Duration::from_millis(10)));
                    continue;
                }
            }

            if self.control.is_cancelled() || self.control.is_paused() {
                continue;
            }
            if !self.first_operation_reported {
                let pacing = self.control.pacing();
                tracing::info!(
                    item_cost,
                    item_rate_per_second = ?pacing.item_rate_per_second,
                    min_interval_ms = pacing.min_interval.as_millis(),
                    "native inventory first operation starting"
                );
                self.first_operation_reported = true;
            }
            self.last_started = Some(Instant::now());
            self.native_operations = self.native_operations.saturating_add(1);
            return BoundaryResult::Proceed;
        }
    }

    fn operations(&self) -> u64 {
        self.native_operations
    }

    fn paused_time(&self) -> Duration {
        self.paused_time
    }
}

fn paced_call<T>(
    boundary: &mut InventoryBoundary<'_>,
    operation: impl FnOnce() -> OpcResult<T>,
) -> Result<T, InventoryError> {
    match boundary.before_operation() {
        BoundaryResult::Proceed => operation().map_err(InventoryError::from),
        BoundaryResult::Cancelled => Err(InventoryError::Cancelled),
    }
}

/// Traverse one server without exposing browse-session state to the caller.
#[allow(clippy::redundant_pub_crate, clippy::too_many_lines)]
pub fn run_inventory<C: ServerConnector>(
    connector: &C,
    server_name: &str,
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
    let mut queue = VecDeque::from([initial_work(capabilities)]);
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
                    queue.push_back(initial_work(terminal.capabilities));
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

fn initial_work(capabilities: BrowseCapabilities) -> BranchWork {
    let location = if capabilities.supports_da3 {
        BranchLocation::Da3(None)
    } else {
        BranchLocation::Da2(Vec::new())
    };
    BranchWork {
        location,
        breadcrumbs: Vec::new(),
        da3_continuation: None,
        da2_state: None,
    }
}

fn is_initial_da3_root(work: &BranchWork) -> bool {
    matches!(work.location, BranchLocation::Da3(None))
        && work.da3_continuation.is_none()
        && work.breadcrumbs.is_empty()
}

fn merge_warning(existing: &mut Option<String>, warning: String) {
    match existing {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&warning);
        }
        None => *existing = Some(warning),
    }
}

fn capabilities_for_inventory<S: ConnectedServer>(
    server: &S,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<BrowseCapabilities, InventoryError> {
    let supports_da3 = server.supports_da3_browse();
    let supports_da2 = server.supports_da2_browse();
    if !supports_da3 && !supports_da2 {
        return Err(OpcError::NotImplemented(
            "Server exposes neither OPC DA 3.0 nor OPC DA 2.x browsing".to_string(),
        )
        .into());
    }
    let namespace = if supports_da2 {
        let organization = match boundary.before_operation() {
            BoundaryResult::Proceed => server.query_organization()?,
            BoundaryResult::Cancelled => return Err(InventoryError::Cancelled),
        };
        match organization {
            value if value == OPC_NS_FLAT.0.cast_unsigned() => {
                crate::provider::BrowseNamespace::Flat
            }
            value if value == crate::bindings::da::OPC_NS_HIERARCHIAL.0.cast_unsigned() => {
                crate::provider::BrowseNamespace::Hierarchical
            }
            value => {
                return Err(OpcError::Server(
                    "Server returned an unknown namespace organization".to_string(),
                    value,
                )
                .into());
            }
        }
    } else {
        crate::provider::BrowseNamespace::Unknown
    };
    Ok(BrowseCapabilities {
        namespace,
        supports_da3,
        supports_da2,
        max_page_size: crate::native_browse::MAX_BROWSE_PAGE_SIZE,
    })
}

fn next_page<S: ConnectedServer>(
    server: &S,
    work: &mut BranchWork,
    batch_size: u32,
    context: &mut InventoryPageContext<'_, '_>,
) -> Result<InventoryPage, InventoryError> {
    match &work.location {
        BranchLocation::Da3(item_id) => {
            let is_root = item_id.is_none() && work.breadcrumbs.is_empty();
            let page = match context.boundary.before_operation_with_cost(batch_size) {
                BoundaryResult::Proceed => server.browse_da3(
                    item_id.as_deref(),
                    work.da3_continuation.as_deref(),
                    batch_size,
                    BrowseNodeFilter::All,
                ),
                BoundaryResult::Cancelled => return Err(InventoryError::Cancelled),
            }
            .map_err(|error| {
                if is_root && is_da3_browse_compatibility_error(&error) {
                    error
                } else {
                    contextual_browse_error(
                        error,
                        "browse_da3",
                        &work.breadcrumbs,
                        item_id.as_deref(),
                    )
                }
            })?;
            let nodes = page
                .elements
                .into_iter()
                .map(map_da3_node)
                .collect::<OpcResult<Vec<_>>>()?;
            let continuation = page
                .continuation
                .filter(|_| page.more_elements)
                .map(InventoryContinuation::Da3);
            if page.more_elements && continuation.is_none() {
                return Err(InventoryError::Failed(OpcError::Internal(
                    "DA3 server reported more elements without a continuation point".to_string(),
                )));
            }
            Ok(InventoryPage {
                nodes,
                continuation,
            })
        }
        BranchLocation::Da2(path) => {
            if work.da2_state.is_none() {
                work.da2_state = Some(start_da2_page(
                    server,
                    path,
                    context.current_da2_path,
                    context.boundary,
                )?);
            }
            let state = work.da2_state.take().ok_or_else(|| {
                OpcError::Internal("DA2 inventory page state disappeared".to_string())
            })?;
            let (nodes, state) = browse_da2_page(server, state, batch_size, context)?;
            Ok(InventoryPage {
                nodes,
                continuation: state.map(|state| InventoryContinuation::Da2(Box::new(state))),
            })
        }
    }
}

fn map_da3_node(element: NativeBrowseElement) -> OpcResult<InventoryNode> {
    let kind = match (element.has_children, element.is_item) {
        (true, true) => BrowseNodeKind::BranchAndItem,
        (true, false) => BrowseNodeKind::Branch,
        (false, true) => BrowseNodeKind::Item,
        (false, false) => {
            return Err(OpcError::Internal(format!(
                "DA3 browse element '{}' is neither a branch nor an item",
                element.name
            )));
        }
    };
    if kind.is_item() && element.item_id.is_none() {
        return Err(OpcError::Internal(format!(
            "DA3 item '{}' did not include an item ID",
            element.name
        )));
    }
    let child = kind
        .has_children()
        .then(|| BranchLocation::Da3(element.item_id.clone()));
    if child.is_some() && element.item_id.is_none() {
        return Err(OpcError::Internal(format!(
            "DA3 branch '{}' did not include an item ID",
            element.name
        )));
    }
    Ok(InventoryNode {
        display_name: element.name,
        item_id: element.item_id,
        kind,
        child,
    })
}

struct Da2PageState {
    parent_path: Vec<String>,
    branches: Option<BufferedBrowseIterator>,
    items: Option<BufferedBrowseIterator>,
    flat: bool,
    merged_items: HashSet<String>,
}

struct InventoryPageContext<'a, 'control> {
    current_da2_path: &'a mut Vec<String>,
    skipped_invalid_branches: &'a mut u64,
    first_skipped_invalid_branch: &'a mut Option<String>,
    skipped_non_progressing_branches: &'a mut u64,
    first_skipped_non_progressing_branch: &'a mut Option<String>,
    boundary: &'a mut InventoryBoundary<'control>,
}

fn start_da2_page<S: ConnectedServer>(
    server: &S,
    parent_path: &[String],
    current_path: &mut Vec<String>,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<Da2PageState, InventoryError> {
    move_to_da2_path(server, current_path, parent_path, boundary)?;
    let flat = match paced_call(boundary, || server.query_organization()) {
        Ok(value) => value == OPC_NS_FLAT.0.cast_unsigned(),
        Err(InventoryError::Failed(error)) => {
            return Err(
                contextual_browse_error(error, "query_organization", parent_path, None).into(),
            );
        }
        Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
    };
    let branches = if flat {
        None
    } else {
        let iterator = paced_call(boundary, || {
            server.begin_da2_browse(OPC_BRANCH.0.cast_unsigned(), Some(""), 0, 0)
        })
        .map_err(|error| match error {
            InventoryError::Failed(error) => InventoryError::Failed(contextual_browse_error(
                error,
                "begin_da2_browse(branches)",
                parent_path,
                None,
            )),
            InventoryError::Cancelled => InventoryError::Cancelled,
        })?;
        Some(BufferedBrowseIterator::new(
            iterator,
            "inventory DA2 branch iterator",
            parent_path,
        ))
    };
    let iterator = paced_call(boundary, || {
        server.begin_da2_browse(
            if flat {
                OPC_FLAT.0.cast_unsigned()
            } else {
                OPC_LEAF.0.cast_unsigned()
            },
            Some(""),
            0,
            0,
        )
    })
    .map_err(|error| match error {
        InventoryError::Failed(error) => InventoryError::Failed(contextual_browse_error(
            error,
            if flat {
                "begin_da2_browse(flat)"
            } else {
                "begin_da2_browse(items)"
            },
            parent_path,
            None,
        )),
        InventoryError::Cancelled => InventoryError::Cancelled,
    })?;
    let items = Some(BufferedBrowseIterator::new(
        iterator,
        if flat {
            "inventory DA2 flat iterator"
        } else {
            "inventory DA2 item iterator"
        },
        parent_path,
    ));
    Ok(Da2PageState {
        parent_path: parent_path.to_vec(),
        branches,
        items,
        flat,
        merged_items: HashSet::new(),
    })
}

fn browse_da2_page<S: ConnectedServer>(
    server: &S,
    mut state: Da2PageState,
    batch_size: u32,
    context: &mut InventoryPageContext<'_, '_>,
) -> Result<(Vec<InventoryNode>, Option<Da2PageState>), InventoryError> {
    move_to_da2_path(
        server,
        context.current_da2_path,
        &state.parent_path,
        context.boundary,
    )?;
    let mut nodes = Vec::with_capacity(batch_size as usize);
    while nodes.len() < batch_size as usize {
        let Some((mut kind, name)) = state
            .next(
                context.boundary,
                context.skipped_non_progressing_branches,
                context.first_skipped_non_progressing_branch,
            )
            .map_err(|error| {
                contextual_inventory_error(error, "enumerate_da2_names", &state.parent_path, None)
            })?
        else {
            break;
        };
        if kind == BrowseNodeKind::Item && state.merged_items.contains(&name) {
            continue;
        }
        let (item_id, child) = match kind {
            BrowseNodeKind::Branch => {
                let Some(mapped) = map_inventory_da2_branch(
                    server,
                    &mut state,
                    &name,
                    context.skipped_invalid_branches,
                    context.first_skipped_invalid_branch,
                    context.boundary,
                )?
                else {
                    continue;
                };
                kind = mapped.kind;
                (mapped.item_id, mapped.child)
            }
            BrowseNodeKind::Item => {
                let item_id = if state.flat {
                    name.clone()
                } else {
                    match paced_call(context.boundary, || server.get_item_id(&name)) {
                        Ok(item_id) => item_id,
                        Err(InventoryError::Failed(error)) => {
                            return Err(contextual_browse_error(
                                error,
                                "get_item_id",
                                &state.parent_path,
                                Some(&name),
                            )
                            .into());
                        }
                        Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
                    }
                };
                let child = if !state.flat
                    && probe_da2_name_has_children(
                        server,
                        &name,
                        &state.parent_path,
                        context.boundary,
                    )? {
                    let mut child_path = state.parent_path.clone();
                    child_path.push(name.clone());
                    kind = BrowseNodeKind::BranchAndItem;
                    Some(BranchLocation::Da2(child_path))
                } else {
                    None
                };
                (Some(item_id), child)
            }
            BrowseNodeKind::BranchAndItem => {
                return Err(InventoryError::Failed(OpcError::Internal(
                    "DA2 browse returned an impossible combined node kind".to_string(),
                )));
            }
        };
        nodes.push(InventoryNode {
            display_name: name,
            item_id,
            kind,
            child,
        });
    }
    let has_more = state.has_more(
        context.boundary,
        context.skipped_non_progressing_branches,
        context.first_skipped_non_progressing_branch,
    )?;
    Ok((nodes, has_more.then_some(state)))
}

fn map_inventory_da2_branch<S: ConnectedServer>(
    server: &S,
    state: &mut Da2PageState,
    name: &str,
    skipped_invalid_branches: &mut u64,
    first_skipped_invalid_branch: &mut Option<String>,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<Option<InventoryDa2BranchNode>, InventoryError> {
    let mut child_path = state.parent_path.clone();
    child_path.push(name.to_string());
    let item_id = match paced_call(boundary, || server.resolve_da2_item_id(name)) {
        Ok(item_id) => item_id,
        Err(InventoryError::Failed(error))
            if crate::opc_da::errors::is_com_hresult(&error, E_INVALIDARG_HRESULT) =>
        {
            None
        }
        Err(InventoryError::Failed(error)) => {
            return Err(contextual_browse_error(
                error,
                "classify_da2_branch(get_item_id)",
                &state.parent_path,
                Some(name),
            )
            .into());
        }
        Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
    };
    let down = OPC_BROWSE_DOWN.0.cast_unsigned();
    let up = OPC_BROWSE_UP.0.cast_unsigned();
    let navigation = match paced_call(boundary, || server.change_browse_position(down, name)) {
        Ok(()) => {
            paced_call(boundary, || server.change_browse_position(up, "")).map_err(|error| {
                contextual_inventory_error(
                    error,
                    "classify_da2_branch(up)",
                    &state.parent_path,
                    Some(name),
                )
            })?;
            Da2BranchNavigation::Navigable
        }
        Err(InventoryError::Failed(error))
            if crate::opc_da::errors::is_com_hresult(&error, E_INVALIDARG_HRESULT) =>
        {
            Da2BranchNavigation::RejectedInvalidArgument
        }
        Err(InventoryError::Failed(error)) => {
            return Err(contextual_browse_error(
                error,
                "classify_da2_branch(down)",
                &state.parent_path,
                Some(name),
            )
            .into());
        }
        Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
    };
    Ok(match (item_id, navigation) {
        (Some(item_id), Da2BranchNavigation::Navigable) => {
            state.merged_items.insert(name.to_string());
            Some(InventoryDa2BranchNode {
                kind: BrowseNodeKind::BranchAndItem,
                item_id: Some(item_id),
                child: Some(BranchLocation::Da2(child_path)),
            })
        }
        (Some(item_id), Da2BranchNavigation::RejectedInvalidArgument) => {
            state.merged_items.insert(name.to_string());
            tracing::debug!(
                browse_path = ?state.parent_path,
                item_name = ?name,
                hresult = "0x80070057",
                "preserving exact DA2 item returned as a non-navigable branch"
            );
            Some(InventoryDa2BranchNode {
                kind: BrowseNodeKind::Item,
                item_id: Some(item_id),
                child: None,
            })
        }
        (None, Da2BranchNavigation::Navigable) => Some(InventoryDa2BranchNode {
            kind: BrowseNodeKind::Branch,
            item_id: None,
            child: Some(BranchLocation::Da2(child_path)),
        }),
        (None, Da2BranchNavigation::RejectedInvalidArgument) => {
            *skipped_invalid_branches = skipped_invalid_branches.saturating_add(1);
            if first_skipped_invalid_branch.is_none() {
                *first_skipped_invalid_branch = Some(format!(
                    "name {name:?} at {}",
                    describe_browse_path(&state.parent_path)
                ));
            }
            tracing::warn!(
                browse_path = ?state.parent_path,
                item_name = ?name,
                hresult = "0x80070057",
                "skipping non-navigable DA2 branch-only name"
            );
            None
        }
    })
}

fn probe_da2_name_has_children<S: ConnectedServer>(
    server: &S,
    item_name: &str,
    path: &[String],
    boundary: &mut InventoryBoundary<'_>,
) -> Result<bool, InventoryError> {
    let down = OPC_BROWSE_DOWN.0.cast_unsigned();
    let up = OPC_BROWSE_UP.0.cast_unsigned();
    match paced_call(boundary, || server.change_browse_position(down, item_name)) {
        Ok(()) => {
            paced_call(boundary, || server.change_browse_position(up, "")).map_err(|error| {
                contextual_inventory_error(error, "probe_da2_branch(up)", path, Some(item_name))
            })?;
            Ok(true)
        }
        Err(InventoryError::Failed(error))
            if !matches!(
                error,
                OpcError::Com { ref source }
                    if matches!(
                        source.code().0.cast_unsigned(),
                        0x8007_06BA | 0x8007_06BF | 0x8007_06BE | 0x8008_0005
                    )
            ) =>
        {
            Ok(false)
        }
        Err(InventoryError::Failed(error)) => {
            Err(
                contextual_browse_error(error, "probe_da2_branch(down)", path, Some(item_name))
                    .into(),
            )
        }
        Err(InventoryError::Cancelled) => Err(InventoryError::Cancelled),
    }
}

impl Da2PageState {
    fn next(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
        skipped_non_progressing_branches: &mut u64,
        first_skipped_non_progressing_branch: &mut Option<String>,
    ) -> Result<Option<(BrowseNodeKind, String)>, InventoryError> {
        let branch_result = self
            .branches
            .as_mut()
            .map(|branches| branches.next(boundary));
        match branch_result {
            Some(Some(Ok(name))) => return Ok(Some((BrowseNodeKind::Branch, name))),
            Some(Some(Err(InventoryError::Failed(error))))
                if is_non_progress_browse_error(&error) =>
            {
                self.skip_non_progressing_branch(
                    &error,
                    skipped_non_progressing_branches,
                    first_skipped_non_progressing_branch,
                );
            }
            Some(Some(Err(error))) => return Err(error),
            Some(None) => self.branches = None,
            None => {}
        }
        let item_result = self.items.as_mut().map(|items| items.next(boundary));
        match item_result {
            Some(Some(Ok(name))) => return Ok(Some((BrowseNodeKind::Item, name))),
            Some(Some(Err(error))) => return Err(error),
            Some(None) => self.items = None,
            None => {}
        }
        Ok(None)
    }

    fn has_more(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
        skipped_non_progressing_branches: &mut u64,
        first_skipped_non_progressing_branch: &mut Option<String>,
    ) -> Result<bool, InventoryError> {
        let branch_has_more = match &mut self.branches {
            Some(branches) => branches.has_more(boundary)?,
            None => false,
        };
        if branch_has_more {
            if let Some(error) = self
                .branches
                .as_mut()
                .and_then(BufferedBrowseIterator::take_non_progress)
            {
                self.skip_non_progressing_branch(
                    &error,
                    skipped_non_progressing_branches,
                    first_skipped_non_progressing_branch,
                );
            } else {
                return Ok(true);
            }
        }

        if let Some(items) = &mut self.items
            && items.has_more(boundary)?
        {
            return Ok(true);
        }
        Ok(false)
    }

    fn skip_non_progressing_branch(
        &mut self,
        error: &OpcError,
        skipped_non_progressing_branches: &mut u64,
        first_skipped_non_progressing_branch: &mut Option<String>,
    ) {
        *skipped_non_progressing_branches = skipped_non_progressing_branches.saturating_add(1);
        if first_skipped_non_progressing_branch.is_none() {
            *first_skipped_non_progressing_branch = Some(format!(
                "DA2 branch iterator at {}",
                describe_browse_path(&self.parent_path)
            ));
        }
        tracing::warn!(
            browse_path = ?self.parent_path,
            error = ?error,
            "skipping non-progressing DA2 branch iterator and continuing with item iterator"
        );
        self.branches = None;
    }
}

struct BufferedBrowseIterator {
    inner: Box<dyn BrowseStringIterator>,
    pending: Option<OpcResult<String>>,
}

impl BufferedBrowseIterator {
    fn new(
        inner: Box<dyn BrowseStringIterator>,
        iterator_type: &str,
        browse_path: &[String],
    ) -> Self {
        Self {
            inner: guard_browse_iterator(inner, iterator_type, browse_path),
            pending: None,
        }
    }

    fn next(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
    ) -> Option<Result<String, InventoryError>> {
        if let Some(value) = self.pending.take() {
            return Some(value.map_err(InventoryError::from));
        }
        match self.next_native(boundary) {
            Ok(value) => value.map(|value| value.map_err(InventoryError::from)),
            Err(error) => Some(Err(error)),
        }
    }

    fn has_more(&mut self, boundary: &mut InventoryBoundary<'_>) -> Result<bool, InventoryError> {
        if self.pending.is_none() {
            self.pending = self.next_native(boundary)?;
        }
        Ok(self.pending.is_some())
    }

    fn next_native(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
    ) -> Result<Option<OpcResult<String>>, InventoryError> {
        let control = boundary.control;
        let mut cancelled = false;
        let mut before_native_operation =
            |item_cost| match boundary.before_operation_with_cost(item_cost) {
                BoundaryResult::Proceed => true,
                BoundaryResult::Cancelled => {
                    cancelled = true;
                    false
                }
            };
        let mut should_cancel = || control.is_cancelled();
        let value = self
            .inner
            .next_string_with_gate(&mut before_native_operation, &mut should_cancel);
        if cancelled || control.is_cancelled() {
            Err(InventoryError::Cancelled)
        } else {
            Ok(value)
        }
    }

    fn take_non_progress(&mut self) -> Option<OpcError> {
        let recoverable = self
            .pending
            .as_ref()
            .is_some_and(|result| result.as_ref().is_err_and(is_non_progress_browse_error));
        if !recoverable {
            return None;
        }
        match self.pending.take() {
            Some(Err(error)) => Some(error),
            _ => None,
        }
    }
}

fn move_to_da2_path<S: ConnectedServer>(
    server: &S,
    current_path: &mut Vec<String>,
    target: &[String],
    boundary: &mut InventoryBoundary<'_>,
) -> Result<(), InventoryError> {
    let shared = current_path
        .iter()
        .zip(target)
        .take_while(|(left, right)| left == right)
        .count();
    for _ in shared..current_path.len() {
        paced_call(boundary, || {
            server.change_browse_position(OPC_BROWSE_UP.0.cast_unsigned(), "")
        })
        .map_err(|error| {
            contextual_inventory_error(
                error,
                "change_browse_position(up)",
                current_path,
                current_path.last().map(String::as_str),
            )
        })?;
    }
    current_path.truncate(shared);
    for branch in &target[shared..] {
        paced_call(boundary, || {
            server.change_browse_position(OPC_BROWSE_DOWN.0.cast_unsigned(), branch)
        })
        .map_err(|error| {
            contextual_inventory_error(
                error,
                "change_browse_position(down)",
                current_path,
                Some(branch),
            )
        })?;
        current_path.push(branch.clone());
    }
    Ok(())
}

fn describe_browse_path(path: &[String]) -> String {
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
fn progress(
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

fn send_event(sender: &mpsc::Sender<OpcResult<InventoryEvent>>, event: InventoryEvent) -> bool {
    sender.blocking_send(Ok(event)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::connector::{ConnectedGroup, RemoteArray, classify_da2_branch};
    use crate::bindings::da::{
        OPC_NS_HIERARCHIAL, tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE,
    };
    use crate::opc_da::errors::{
        E_INVALIDARG_HRESULT, E_NOTIMPL_HRESULT, RPC_X_NULL_REF_POINTER_HRESULT,
    };
    use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::Win32::System::Variant::VARIANT;
    use windows::core::HRESULT;

    struct GateAwareIterator {
        items: VecDeque<String>,
        refill_size: u32,
        remaining_in_refill: usize,
        costs: Arc<Mutex<Vec<u32>>>,
        done: bool,
    }

    impl BrowseStringIterator for GateAwareIterator {
        fn next_string(&mut self) -> Option<OpcResult<String>> {
            self.items.pop_front().map(Ok)
        }

        fn next_string_with_gate(
            &mut self,
            before_native_operation: &mut dyn FnMut(u32) -> bool,
            should_cancel: &mut dyn FnMut() -> bool,
        ) -> Option<OpcResult<String>> {
            if self.done || should_cancel() {
                return None;
            }
            if self.remaining_in_refill == 0 {
                if !before_native_operation(self.refill_size) {
                    return None;
                }
                self.costs.lock().unwrap().push(self.refill_size);
                if should_cancel() {
                    return None;
                }
                self.remaining_in_refill = self.items.len().min(self.refill_size as usize);
                if self.remaining_in_refill == 0 {
                    self.done = true;
                    return None;
                }
            }

            if should_cancel() {
                return None;
            }
            self.remaining_in_refill -= 1;
            Some(Ok(self.items.pop_front().expect(
                "remaining_in_refill must match the number of queued items",
            )))
        }
    }

    struct TestGroup;

    impl ConnectedGroup for TestGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(RemoteArray<tagOPCITEMRESULT>, RemoteArray<HRESULT>)> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[ItemHandle],
        ) -> OpcResult<(RemoteArray<tagOPCITEMSTATE>, RemoteArray<HRESULT>)> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn write(
            &self,
            _server_handles: &[ItemHandle],
            _values: &[VARIANT],
        ) -> OpcResult<RemoteArray<HRESULT>> {
            Err(OpcError::NotImplemented("test".to_string()))
        }
    }

    struct Da3Server {
        total: usize,
        browse_calls: Arc<AtomicUsize>,
        batch_sizes: Arc<Mutex<Vec<u32>>>,
        fail: bool,
        da3_hresult: Option<u32>,
        supports_da2: bool,
        da2_items: Vec<String>,
    }

    impl ConnectedServer for Da3Server {
        type Group = TestGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(OPC_NS_FLAT.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<crate::backend::connector::StringIterator> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn supports_da2_browse(&self) -> bool {
            self.supports_da2
        }

        fn supports_da3_browse(&self) -> bool {
            true
        }

        fn browse_da3(
            &self,
            _item_id: Option<&str>,
            continuation: Option<&str>,
            max_elements: u32,
            _filter: BrowseNodeFilter,
        ) -> OpcResult<crate::backend::connector::NativeBrowsePage> {
            self.browse_calls.fetch_add(1, Ordering::Relaxed);
            self.batch_sizes.lock().unwrap().push(max_elements);
            if let Some(hresult) = self.da3_hresult {
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(hresult.cast_signed())),
                });
            }
            if self.fail {
                return Err(OpcError::Internal("synthetic browse failure".to_string()));
            }
            let start = continuation
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let end = (start + max_elements as usize).min(self.total);
            let elements = (start..end)
                .map(|index| NativeBrowseElement {
                    name: format!("Item{index}"),
                    item_id: Some(format!("exact::{index}")),
                    has_children: false,
                    is_item: true,
                })
                .collect();
            Ok(crate::backend::connector::NativeBrowsePage {
                elements,
                more_elements: end < self.total,
                continuation: (end < self.total).then(|| end.to_string()),
            })
        }

        fn begin_da2_browse(
            &self,
            browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            if browse_type != OPC_FLAT.0.cast_unsigned() {
                return Err(OpcError::InvalidState(
                    "fallback test expected a flat DA2 browse".to_string(),
                ));
            }
            Ok(Box::new(self.da2_items.clone().into_iter().map(Ok)))
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Ok(())
        }
    }

    fn collect(
        receiver: &mut mpsc::Receiver<OpcResult<InventoryEvent>>,
    ) -> (
        Vec<InventoryEntry>,
        Option<InventoryCompleted>,
        Option<OpcError>,
    ) {
        let mut entries = Vec::new();
        let mut completed = None;
        let mut error = None;
        while let Ok(message) = receiver.try_recv() {
            match message {
                Ok(InventoryEvent::Entry(entry)) => entries.push(entry),
                Ok(InventoryEvent::Completed(result)) => completed = Some(result),
                Ok(InventoryEvent::Progress(_) | InventoryEvent::Slice(_)) => {}
                Err(value) => error = Some(value),
            }
        }
        (entries, completed, error)
    }

    struct SharedConnector<S> {
        server: Arc<Mutex<Option<S>>>,
    }

    impl<S> ServerConnector for SharedConnector<S>
    where
        S: ConnectedServer + Send + 'static,
    {
        type Server = S;

        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(Vec::new())
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            self.server
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| OpcError::Internal("server already connected".to_string()))
        }
    }

    #[test]
    fn inventory_handles_more_than_one_hundred_thousand_entries() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 100_001,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(100_300);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 1_000,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();
        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(entries.len(), 100_001);
        assert!(completed.is_some_and(|value| value.complete));
        assert!(error.is_none());
    }

    #[test]
    fn zero_max_entries_emits_no_entries() {
        let calls = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 1,
                browse_calls: Arc::clone(&calls),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(8);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 100,
                max_entries: Some(0),
            },
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();
        let (entries, completed, error) = collect(&mut receiver);
        assert!(entries.is_empty());
        assert!(completed.is_some_and(|value| value.truncated));
        assert!(error.is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancellation_stops_before_the_next_page() {
        let calls = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 10,
                browse_calls: Arc::clone(&calls),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let control = InventoryControl::new();
        control.cancel();
        let (sender, mut receiver) = mpsc::channel(8);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions::default(),
            &control,
            &sender,
        )
        .unwrap();
        let (entries, completed, error) = collect(&mut receiver);
        assert!(entries.is_empty());
        assert!(completed.is_some_and(|value| value.cancelled));
        assert!(error.is_none());
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn browse_errors_are_terminal_typed_errors() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 1,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: true,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(8);
        let result = run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions::default(),
            &InventoryControl::new(),
            &sender,
        );
        assert!(matches!(
            result,
            Err(OpcError::Internal(message)) if message.contains("synthetic browse failure")
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(Ok(InventoryEvent::Progress(_)))
        ));
    }

    #[test]
    fn da3_root_compatibility_failures_fall_back_to_da2_inventory() {
        for hresult in [RPC_X_NULL_REF_POINTER_HRESULT, E_NOTIMPL_HRESULT] {
            let connector = Arc::new(SharedConnector {
                server: Arc::new(Mutex::new(Some(Da3Server {
                    total: 0,
                    browse_calls: Arc::new(AtomicUsize::new(0)),
                    batch_sizes: Arc::new(Mutex::new(Vec::new())),
                    fail: false,
                    da3_hresult: Some(hresult),
                    supports_da2: true,
                    da2_items: vec!["Channel.Device.Tag".to_string()],
                }))),
            });
            let (sender, mut receiver) = mpsc::channel(16);

            run_inventory(
                connector.as_ref(),
                "test",
                InventoryOptions::default(),
                &InventoryControl::new(),
                &sender,
            )
            .unwrap();

            let (entries, completed, error) = collect(&mut receiver);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].item_id, "Channel.Device.Tag");
            assert!(completed.is_some_and(|value| {
                value.complete
                    && !value.capabilities.supports_da3
                    && value.capabilities.supports_da2
                    && value.warning.is_some_and(|warning| {
                        warning.contains(&format!("0x{hresult:08X}"))
                            && warning.contains("continued through OPC DA 2.x")
                    })
            }));
            assert!(error.is_none());
        }
    }

    #[test]
    fn paused_inventory_makes_no_browse_call_until_resumed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 1,
                browse_calls: Arc::clone(&calls),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let control = InventoryControl::new();
        control.pause();
        let worker_control = control.clone();
        let (sender, mut receiver) = mpsc::channel(8);
        let worker = std::thread::spawn(move || {
            run_inventory(
                connector.as_ref(),
                "test",
                InventoryOptions::default(),
                &worker_control,
                &sender,
            )
            .unwrap();
        });

        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(calls.load(Ordering::Acquire), 0);
        control.resume();
        worker.join().unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let mut saw_slice = false;
        while let Ok(event) = receiver.try_recv() {
            if matches!(event, Ok(InventoryEvent::Slice(_))) {
                saw_slice = true;
            }
        }
        assert!(saw_slice);
    }

    #[test]
    fn pacing_updates_are_seen_at_the_next_boundary() {
        let control = InventoryControl::new();
        control.set_pacing(crate::provider::InventoryPacing {
            min_interval: Duration::from_millis(100),
            ..Default::default()
        });
        let mut boundary = InventoryBoundary::new(&control);
        assert_eq!(boundary.before_operation(), BoundaryResult::Proceed);
        let updater = {
            let control = control.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                control.set_pacing(crate::provider::InventoryPacing {
                    min_interval: Duration::ZERO,
                    ..Default::default()
                });
            })
        };
        let started = Instant::now();
        assert_eq!(boundary.before_operation(), BoundaryResult::Proceed);
        updater.join().unwrap();
        assert!(started.elapsed() < Duration::from_millis(90));
    }

    #[test]
    fn item_rate_pacing_charges_the_requested_native_batch() {
        let pacing = crate::provider::InventoryPacing {
            min_interval: Duration::ZERO,
            item_rate_per_second: Some(50),
        };
        assert_eq!(pacing_interval(pacing, 100), Duration::from_secs(2));
        assert_eq!(pacing_interval(pacing, 0), Duration::from_millis(20));
        assert_eq!(
            pacing_interval(
                crate::provider::InventoryPacing {
                    min_interval: Duration::from_millis(100),
                    item_rate_per_second: Some(50),
                },
                1,
            ),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn da2_iterator_pacing_charges_each_native_refill_not_each_cached_item() {
        let costs = Arc::new(Mutex::new(Vec::new()));
        let mut iterator = BufferedBrowseIterator::new(
            Box::new(GateAwareIterator {
                items: ["Item1", "Item2", "Item3"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                refill_size: 256,
                remaining_in_refill: 0,
                costs: Arc::clone(&costs),
                done: false,
            }),
            "test iterator",
            &[],
        );
        let control = InventoryControl::new();
        let mut boundary = InventoryBoundary::new(&control);
        let mut values = Vec::new();

        while let Some(value) = iterator.next(&mut boundary) {
            values.push(value.expect("the test item must be valid"));
        }

        assert_eq!(
            values,
            vec![
                "Item1".to_string(),
                "Item2".to_string(),
                "Item3".to_string()
            ]
        );
        assert_eq!(*costs.lock().unwrap(), vec![256, 256]);
        assert_eq!(boundary.operations(), 2);
    }

    #[test]
    fn batch_size_updates_are_seen_at_the_next_slice_boundary() {
        let calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 3,
                browse_calls: Arc::clone(&calls),
                batch_sizes: Arc::clone(&batch_sizes),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let control = InventoryControl::new();
        control.set_pacing(crate::provider::InventoryPacing {
            min_interval: Duration::from_millis(500),
            ..Default::default()
        });
        let worker_control = control.clone();
        let (sender, _receiver) = mpsc::channel(16);
        let worker = std::thread::spawn(move || {
            run_inventory(
                connector.as_ref(),
                "test",
                InventoryOptions {
                    batch_size: 1,
                    max_entries: None,
                },
                &worker_control,
                &sender,
            )
            .unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        // The first call is the DA3 capability probe; wait for the first
        // inventory slice before changing the batch size.
        while calls.load(Ordering::Acquire) < 2 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(control.set_batch_size(0).is_err());
        assert!(
            control
                .set_batch_size(crate::provider::MAX_INVENTORY_BATCH_SIZE + 1)
                .is_err()
        );
        control.set_batch_size(2).unwrap();
        control.set_pacing(crate::provider::InventoryPacing::default());
        worker.join().unwrap();

        // The capability probe is the first bounded DA3 call; the following
        // two values are the inventory slices before and after the update.
        assert_eq!(*batch_sizes.lock().unwrap(), vec![1, 1, 2]);
    }

    #[test]
    fn each_native_page_emits_a_typed_slice_observation() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 3,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: None,
                supports_da2: false,
                da2_items: Vec::new(),
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(16);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 2,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();
        let mut slices = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let Ok(InventoryEvent::Slice(slice)) = event {
                slices.push(slice);
            }
        }
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].backend, InventorySliceBackend::Da3);
        assert_eq!(slices[0].nodes_returned, 2);
        assert_eq!(slices[0].native_operations, 1);
        assert_eq!(slices[1].sequence, 2);
    }

    #[test]
    fn duplicate_da3_item_ids_are_emitted_once_and_branch_items_are_selectable() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(DuplicateDa3Server))),
        });
        let (sender, mut receiver) = mpsc::channel(16);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions::default(),
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();
        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["same", "branch-item"]
        );
        assert_eq!(entries[1].kind, BrowseNodeKind::BranchAndItem);
        assert!(completed.is_some_and(|value| value.complete));
        assert!(error.is_none());
    }

    #[test]
    fn da2_branch_and_item_is_emitted_once_and_children_are_traversed() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da2SemanticsServer::default()))),
        });
        let (sender, mut receiver) = mpsc::channel(16);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 1,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();
        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["Pump", "Pressure", "Pump.PV"]
        );
        assert_eq!(entries[0].kind, BrowseNodeKind::BranchAndItem);
        assert!(completed.is_some_and(|value| value.complete));
        assert!(error.is_none());
    }

    #[test]
    fn da2_branch_only_navigation_rejection_is_skipped_without_losing_items() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(InvalidDa2BranchServer::default()))),
        });
        let (sender, mut receiver) = mpsc::channel(32);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 10,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();

        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["FCS0528.LeafOnly", "FCS0528.PV", "FCS0528!Odd.PV"]
        );
        assert_eq!(entries[0].kind, BrowseNodeKind::Item);
        assert!(completed.is_some_and(|value| {
            value.complete
                && value.warning.is_some_and(|warning| {
                    warning.contains("skipped 1 non-navigable DA2 branch name(s)")
                        && warning.contains("\"\\u{1}\"")
                        && warning.contains("\"FCS0528\"")
                })
        }));
        assert!(error.is_none());
    }

    #[test]
    fn da2_non_progressing_branch_iterator_is_skipped_without_losing_items() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(InvalidDa2BranchServer {
                non_progressing_branch: true,
                ..Default::default()
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(128);
        run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions::default(),
            &InventoryControl::new(),
            &sender,
        )
        .unwrap();

        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["FCS0528.PV", "FCS0528.LeafOnly"]
        );
        assert!(completed.is_some_and(|value| {
            value.complete
                && value.warning.is_some_and(|warning| {
                    warning.contains("skipped 1 non-progressing DA2 branch iterator(s)")
                        && warning.contains("\"FCS0528\"")
                })
        }));
        assert!(error.is_none());
    }

    #[test]
    fn da2_item_non_progress_is_terminal() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(InvalidDa2BranchServer {
                non_progressing_items: true,
                ..Default::default()
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(128);
        let result = run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions {
                batch_size: 10,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        );

        assert!(matches!(
            result,
            Err(OpcError::BrowseNonProgress { iterator_type, .. })
                if iterator_type == "inventory DA2 item iterator"
        ));
        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["FCS0528.LeafOnly", "FCS0528.PV"]
        );
        assert!(completed.is_none());
        assert!(error.is_none());
    }

    #[test]
    fn da2_unrelated_branch_error_is_terminal() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(InvalidDa2BranchServer {
                fatal_branch: true,
                ..Default::default()
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(32);
        let result = run_inventory(
            connector.as_ref(),
            "test",
            InventoryOptions::default(),
            &InventoryControl::new(),
            &sender,
        );

        assert!(matches!(
            result,
            Err(OpcError::Internal(message))
                if message.contains("classify_da2_branch(down)")
                    && message.contains("\"FCS0528\"")
                    && message.contains("item \"Denied\"")
        ));
        let (_, completed, error) = collect(&mut receiver);
        assert!(completed.is_none());
        assert!(error.is_none());
    }

    #[test]
    fn da2_has_more_discards_prefetched_branch_non_progress_and_uses_items() {
        let control = InventoryControl::new();
        let mut boundary = InventoryBoundary::new(&control);
        let mut state = Da2PageState {
            parent_path: vec!["FCS0528".to_string()],
            branches: Some(BufferedBrowseIterator::new(
                Box::new(std::iter::repeat_with(|| {
                    Ok::<String, OpcError>("\u{1}".to_string())
                })),
                "enumerate_da2_names.branches",
                &["FCS0528".to_string()],
            )),
            items: Some(BufferedBrowseIterator::new(
                Box::new(std::iter::once(Ok::<String, OpcError>("PV".to_string()))),
                "enumerate_da2_names.items",
                &["FCS0528".to_string()],
            )),
            flat: false,
            merged_items: HashSet::new(),
        };
        for _ in 0..63 {
            assert!(matches!(
                state.branches.as_mut().unwrap().next(&mut boundary),
                Some(Ok(value)) if value == "\u{1}"
            ));
        }
        let mut skipped = 0;
        let mut first_skipped = None;

        assert!(
            state
                .has_more(&mut boundary, &mut skipped, &mut first_skipped)
                .unwrap()
        );
        assert!(state.branches.is_none());
        assert_eq!(skipped, 1);
        assert_eq!(
            first_skipped.as_deref(),
            Some("DA2 branch iterator at \"FCS0528\"")
        );
        assert_eq!(
            state
                .next(&mut boundary, &mut skipped, &mut first_skipped)
                .unwrap(),
            Some((BrowseNodeKind::Item, "PV".to_string()))
        );
    }

    #[test]
    fn da2_branch_navigation_propagates_non_invalidarg_errors() {
        let server = InvalidDa2BranchServer::default();
        server
            .change_browse_position(OPC_BROWSE_DOWN.0.cast_unsigned(), "FCS0528")
            .unwrap();

        assert!(matches!(
            classify_da2_branch(&server, "Denied"),
            Err(OpcError::Com { source }) if source.code().0.cast_unsigned() == 0x8007_0005
        ));
    }

    struct DuplicateDa3Server;

    impl ConnectedServer for DuplicateDa3Server {
        type Group = TestGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(OPC_NS_FLAT.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<crate::backend::connector::StringIterator> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn supports_da2_browse(&self) -> bool {
            false
        }

        fn supports_da3_browse(&self) -> bool {
            true
        }

        fn browse_da3(
            &self,
            item_id: Option<&str>,
            _continuation: Option<&str>,
            _max_elements: u32,
            _filter: BrowseNodeFilter,
        ) -> OpcResult<crate::backend::connector::NativeBrowsePage> {
            if item_id.is_some() {
                return Ok(crate::backend::connector::NativeBrowsePage {
                    elements: Vec::new(),
                    more_elements: false,
                    continuation: None,
                });
            }
            Ok(crate::backend::connector::NativeBrowsePage {
                elements: vec![
                    NativeBrowseElement {
                        name: "First".to_string(),
                        item_id: Some("same".to_string()),
                        has_children: false,
                        is_item: true,
                    },
                    NativeBrowseElement {
                        name: "Duplicate".to_string(),
                        item_id: Some("same".to_string()),
                        has_children: false,
                        is_item: true,
                    },
                    NativeBrowseElement {
                        name: "BranchItem".to_string(),
                        item_id: Some("branch-item".to_string()),
                        has_children: true,
                        is_item: true,
                    },
                ],
                more_elements: false,
                continuation: None,
            })
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Da2SemanticsServer {
        position: Mutex<Vec<String>>,
    }

    impl ConnectedServer for Da2SemanticsServer {
        type Group = TestGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(OPC_NS_HIERARCHIAL.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<crate::backend::connector::StringIterator> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
            let mut position = self.position.lock().unwrap();
            if direction == OPC_BROWSE_DOWN.0.cast_unsigned() {
                position.push(name.to_string());
            } else if direction == OPC_BROWSE_UP.0.cast_unsigned() {
                position.pop();
            }
            drop(position);
            Ok(())
        }

        fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
            let position = self.position.lock().unwrap();
            match (position.as_slice(), item_name) {
                ([], "Pump" | "Pressure") => Ok(item_name.to_string()),
                ([pump], "PV") if pump == "Pump" => Ok("Pump.PV".to_string()),
                _ => Err(OpcError::InvalidState("not an item".to_string())),
            }
        }

        fn resolve_da2_item_id(&self, item_name: &str) -> OpcResult<Option<String>> {
            let position = self.position.lock().unwrap();
            Ok((position.is_empty() && item_name == "Pump").then(|| "Pump".to_string()))
        }

        fn da2_name_has_children(&self, item_name: &str) -> OpcResult<bool> {
            let position = self.position.lock().unwrap();
            Ok(position.is_empty() && item_name == "Pump")
        }

        fn supports_da3_browse(&self) -> bool {
            false
        }

        fn begin_da2_browse(
            &self,
            browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            let position = self.position.lock().unwrap().clone();
            let values = match (browse_type, position.as_slice()) {
                (value, []) if value == OPC_BRANCH.0.cast_unsigned() => vec!["Pump"],
                (value, []) if value == OPC_LEAF.0.cast_unsigned() => vec!["Pump", "Pressure"],
                (value, [pump]) if value == OPC_BRANCH.0.cast_unsigned() && pump == "Pump" => {
                    Vec::new()
                }
                (value, [pump]) if value == OPC_LEAF.0.cast_unsigned() && pump == "Pump" => {
                    vec!["PV"]
                }
                _ => Vec::new(),
            };
            Ok(Box::new(values.into_iter().map(str::to_string).map(Ok)))
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct InvalidDa2BranchServer {
        position: Mutex<Vec<String>>,
        non_progressing_branch: bool,
        non_progressing_items: bool,
        fatal_branch: bool,
    }

    impl ConnectedServer for InvalidDa2BranchServer {
        type Group = TestGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(OPC_NS_HIERARCHIAL.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<crate::backend::connector::StringIterator> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
            if direction == OPC_BROWSE_DOWN.0.cast_unsigned() {
                if matches!(name, "\u{1}" | "LeafOnly") {
                    return Err(OpcError::Com {
                        source: windows::core::Error::from_hresult(HRESULT(
                            E_INVALIDARG_HRESULT.cast_signed(),
                        )),
                    });
                }
                if name == "Denied" {
                    return Err(OpcError::Com {
                        source: windows::core::Error::from_hresult(HRESULT(
                            0x8007_0005_u32.cast_signed(),
                        )),
                    });
                }
            }
            let mut position = self.position.lock().unwrap();
            if direction == OPC_BROWSE_DOWN.0.cast_unsigned() {
                position.push(name.to_string());
            } else if direction == OPC_BROWSE_UP.0.cast_unsigned() {
                position.pop();
            }
            drop(position);
            Ok(())
        }

        fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
            let position = self.position.lock().unwrap();
            match (position.as_slice(), item_name) {
                ([], "FCS0528") => Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(
                        0xC004_0007_u32.cast_signed(),
                    )),
                }),
                ([area], "PV") if area == "FCS0528" => Ok("FCS0528.PV".to_string()),
                ([area], "LeafOnly") if area == "FCS0528" => Ok("FCS0528.LeafOnly".to_string()),
                ([area], "\u{1}" | "Denied") if area == "FCS0528" => Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(
                        0xC004_0007_u32.cast_signed(),
                    )),
                }),
                ([area], "Odd") if area == "FCS0528" => Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(
                        E_INVALIDARG_HRESULT.cast_signed(),
                    )),
                }),
                ([area, branch], "PV") if area == "FCS0528" && branch == "Odd" => {
                    Ok("FCS0528!Odd.PV".to_string())
                }
                _ => Err(OpcError::InvalidState("not an item".to_string())),
            }
        }

        fn da2_name_has_children(&self, item_name: &str) -> OpcResult<bool> {
            let position = self.position.lock().unwrap();
            Ok(position.as_slice() == ["FCS0528"] && item_name == "Odd")
        }

        fn supports_da3_browse(&self) -> bool {
            false
        }

        fn begin_da2_browse(
            &self,
            browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            let position = self.position.lock().unwrap().clone();
            let values = if browse_type == OPC_BRANCH.0.cast_unsigned() {
                match position.as_slice() {
                    [] => vec!["FCS0528".to_string()],
                    [area] if area == "FCS0528" && self.fatal_branch => {
                        vec!["Denied".to_string()]
                    }
                    [area] if area == "FCS0528" && self.non_progressing_branch => {
                        return Ok(Box::new(std::iter::repeat_with(|| {
                            Ok::<String, OpcError>("\u{1}".to_string())
                        })));
                    }
                    [area] if area == "FCS0528" => {
                        vec![
                            "\u{1}".to_string(),
                            "Odd".to_string(),
                            "LeafOnly".to_string(),
                        ]
                    }
                    _ => Vec::new(),
                }
            } else if browse_type == OPC_LEAF.0.cast_unsigned() {
                match position.as_slice() {
                    [area] if area == "FCS0528" && self.non_progressing_items => {
                        return Ok(Box::new(std::iter::repeat_with(|| {
                            Ok::<String, OpcError>("PV".to_string())
                        })));
                    }
                    [area] if area == "FCS0528" => {
                        vec!["PV".to_string(), "LeafOnly".to_string()]
                    }
                    [area, branch] if area == "FCS0528" && branch == "Odd" => {
                        vec!["PV".to_string()]
                    }
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            Ok(Box::new(values.into_iter().map(Ok)))
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("test".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Ok(())
        }
    }
}
