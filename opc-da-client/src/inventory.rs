//! Bounded namespace inventory traversal used by the bridge search index.

use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, NativeBrowseElement, ServerConnector,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT,
};
use crate::native_browse::capabilities_for_server;
use crate::opc_da::errors::{OpcError, OpcResult};
use crate::provider::{
    BrowseCapabilities, BrowseNodeFilter, BrowseNodeKind, InventoryCompleted, InventoryControl,
    InventoryEntry, InventoryEvent, InventoryOptions, InventoryProgress,
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
    Da2(Da2PageState),
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
    if options.batch_size == 0 || options.batch_size > 1_000 {
        return Err(OpcError::InvalidState(
            "Inventory batch size must be between 1 and 1000".to_string(),
        ));
    }

    let connected = connector.connect(server_name)?;
    let capabilities = capabilities_for_server(&connected)?;
    let mut queue = VecDeque::from([initial_work(capabilities)]);
    let mut seen_items = HashSet::new();
    let mut current_da2_path = Vec::new();
    let mut branches_visited = 0_u64;
    let mut entries_seen = 0_u64;
    let mut active_time = Duration::ZERO;
    let mut paused_time = Duration::ZERO;

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
            paused_time,
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
        if !wait_until_resumed(control, &mut paused_time) {
            terminal.complete = false;
            terminal.cancelled = true;
            break;
        }

        if work.da3_continuation.is_none() && work.da2_state.is_none() {
            branches_visited = branches_visited.saturating_add(1);
        }
        let call_started = Instant::now();
        let page = match next_page(
            &connected,
            &mut work,
            options.batch_size,
            &mut current_da2_path,
        ) {
            Ok(page) => page,
            Err(error) => {
                let _ = send_event(
                    sender,
                    InventoryEvent::Progress(progress(
                        branches_visited,
                        entries_seen,
                        seen_items.len() as u64,
                        active_time,
                        paused_time,
                    )),
                );
                return Err(error);
            }
        };
        active_time += call_started.elapsed();
        entries_seen = entries_seen.saturating_add(page.nodes.len() as u64);

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
                    terminal.warning = Some("inventory entry limit reached".to_string());
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

        if terminal.cancelled || terminal.truncated {
            break;
        }

        if let Some(continuation) = page.continuation {
            match continuation {
                InventoryContinuation::Da3(continuation) => {
                    work.da3_continuation = Some(continuation);
                }
                InventoryContinuation::Da2(state) => {
                    work.da2_state = Some(state);
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
                paused_time,
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

fn next_page<S: ConnectedServer>(
    server: &S,
    work: &mut BranchWork,
    batch_size: u32,
    current_da2_path: &mut Vec<String>,
) -> OpcResult<InventoryPage> {
    match &work.location {
        BranchLocation::Da3(item_id) => {
            let page = server.browse_da3(
                item_id.as_deref(),
                work.da3_continuation.as_deref(),
                batch_size,
                BrowseNodeFilter::All,
            )?;
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
                return Err(OpcError::Internal(
                    "DA3 server reported more elements without a continuation point".to_string(),
                ));
            }
            Ok(InventoryPage {
                nodes,
                continuation,
            })
        }
        BranchLocation::Da2(path) => {
            if work.da2_state.is_none() {
                work.da2_state = Some(start_da2_page(server, path, current_da2_path)?);
            }
            let state = work.da2_state.take().ok_or_else(|| {
                OpcError::Internal("DA2 inventory page state disappeared".to_string())
            })?;
            let (nodes, state) = browse_da2_page(server, state, batch_size, current_da2_path)?;
            Ok(InventoryPage {
                nodes,
                continuation: state.map(InventoryContinuation::Da2),
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

fn start_da2_page<S: ConnectedServer>(
    server: &S,
    parent_path: &[String],
    current_path: &mut Vec<String>,
) -> OpcResult<Da2PageState> {
    move_to_da2_path(server, current_path, parent_path)?;
    let flat = server.query_organization()? == OPC_NS_FLAT.0.cast_unsigned();
    let branches = if flat {
        None
    } else {
        Some(BufferedBrowseIterator::new(server.begin_da2_browse(
            OPC_BRANCH.0.cast_unsigned(),
            Some(""),
            0,
            0,
        )?))
    };
    let items = Some(BufferedBrowseIterator::new(server.begin_da2_browse(
        if flat {
            OPC_FLAT.0.cast_unsigned()
        } else {
            OPC_LEAF.0.cast_unsigned()
        },
        Some(""),
        0,
        0,
    )?));
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
    current_path: &mut Vec<String>,
) -> OpcResult<(Vec<InventoryNode>, Option<Da2PageState>)> {
    move_to_da2_path(server, current_path, &state.parent_path)?;
    let mut nodes = Vec::with_capacity(batch_size as usize);
    while nodes.len() < batch_size as usize {
        let Some((mut kind, name)) = state.next()? else {
            break;
        };
        if kind == BrowseNodeKind::Item && state.merged_items.contains(&name) {
            continue;
        }
        let (item_id, child) = match kind {
            BrowseNodeKind::Branch => {
                let mut child_path = state.parent_path.clone();
                child_path.push(name.clone());
                let item_id = match server.resolve_da2_item_id(&name)? {
                    Some(item_id) => {
                        state.merged_items.insert(name.clone());
                        kind = BrowseNodeKind::BranchAndItem;
                        Some(item_id)
                    }
                    None => None,
                };
                (item_id, Some(BranchLocation::Da2(child_path)))
            }
            BrowseNodeKind::Item => {
                let item_id = if state.flat {
                    name.clone()
                } else {
                    server.get_item_id(&name)?
                };
                let child = if !state.flat && server.da2_name_has_children(&name)? {
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
                return Err(OpcError::Internal(
                    "DA2 browse returned an impossible combined node kind".to_string(),
                ));
            }
        };
        nodes.push(InventoryNode {
            display_name: name,
            item_id,
            kind,
            child,
        });
    }
    let continuation = state.has_more().then_some(state);
    Ok((nodes, continuation))
}

impl Da2PageState {
    fn next(&mut self) -> OpcResult<Option<(BrowseNodeKind, String)>> {
        if let Some(branches) = &mut self.branches {
            match branches.next() {
                Some(Ok(name)) => return Ok(Some((BrowseNodeKind::Branch, name))),
                Some(Err(error)) => return Err(error),
                None => self.branches = None,
            }
        }
        if let Some(items) = &mut self.items {
            match items.next() {
                Some(Ok(name)) => return Ok(Some((BrowseNodeKind::Item, name))),
                Some(Err(error)) => return Err(error),
                None => self.items = None,
            }
        }
        Ok(None)
    }

    fn has_more(&mut self) -> bool {
        self.branches
            .as_mut()
            .is_some_and(BufferedBrowseIterator::has_more)
            || self
                .items
                .as_mut()
                .is_some_and(BufferedBrowseIterator::has_more)
    }
}

struct BufferedBrowseIterator {
    inner: Box<dyn BrowseStringIterator>,
    pending: Option<OpcResult<String>>,
}

impl BufferedBrowseIterator {
    fn new(inner: Box<dyn BrowseStringIterator>) -> Self {
        Self {
            inner,
            pending: None,
        }
    }

    fn next(&mut self) -> Option<OpcResult<String>> {
        self.pending.take().or_else(|| self.inner.next_string())
    }

    fn has_more(&mut self) -> bool {
        if self.pending.is_none() {
            self.pending = self.inner.next_string();
        }
        self.pending.is_some()
    }
}

fn move_to_da2_path<S: ConnectedServer>(
    server: &S,
    current_path: &mut Vec<String>,
    target: &[String],
) -> OpcResult<()> {
    let shared = current_path
        .iter()
        .zip(target)
        .take_while(|(left, right)| left == right)
        .count();
    for _ in shared..current_path.len() {
        server.change_browse_position(OPC_BROWSE_UP.0.cast_unsigned(), "")?;
    }
    current_path.truncate(shared);
    for branch in &target[shared..] {
        server.change_browse_position(OPC_BROWSE_DOWN.0.cast_unsigned(), branch)?;
        current_path.push(branch.clone());
    }
    Ok(())
}

fn wait_until_resumed(control: &InventoryControl, paused_time: &mut Duration) -> bool {
    let pause_started = Instant::now();
    while control.is_paused() && !control.is_cancelled() {
        std::thread::sleep(Duration::from_millis(25));
    }
    *paused_time += pause_started.elapsed();
    !control.is_cancelled()
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
    use crate::backend::connector::{ConnectedGroup, RemoteArray};
    use crate::bindings::da::{
        OPC_NS_HIERARCHIAL, tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE,
    };
    use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::Win32::System::Variant::VARIANT;
    use windows::core::HRESULT;

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
        fail: bool,
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
            false
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
                Ok(InventoryEvent::Progress(_)) => {}
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
                fail: false,
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(100_010);
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
                fail: false,
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
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 10,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                fail: false,
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
    }

    #[test]
    fn browse_errors_are_terminal_typed_errors() {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 1,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                fail: true,
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
}
