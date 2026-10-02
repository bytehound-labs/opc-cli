//! Native inventory traversal and lifecycle characterization.

use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, NativeBrowseElement, ServerConnector,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT,
};
use crate::errors::{MAX_CONSECUTIVE_EMPTY_DA3_PAGES, OpcError, OpcResult};
use crate::inventory::da2::Da2PageState;
use crate::inventory::iterator::BufferedBrowseIterator;
use crate::inventory::progress::record_skipped_invalid_branch;
use crate::inventory_boundary::{BoundaryResult, InventoryBoundary, pacing_interval};
use crate::inventory_telemetry::{install_native_operation_telemetry, record_native_operation};
use crate::provider::{
    BrowseNodeFilter, BrowseNodeKind, InventoryCompleted, InventoryControl, InventoryEntry,
    InventoryEvent, InventoryNativeOperationKind, InventoryOptions, InventorySliceBackend,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

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

#[test]
fn inventory_events_keep_the_module_target() {
    crate::tests::tracing::assert_event_targets("opc_da_client::inventory", || {
        record_skipped_invalid_branch(
            &mut 0,
            &mut None,
            &["Parent!With/Punctuation".to_string()],
            "Branch.With/Punctuation",
        );
        let control = InventoryControl::new();
        let mut boundary = InventoryBoundary::new(&control);
        assert_eq!(
            boundary.before_operation_with_cost(1),
            BoundaryResult::Proceed
        );
    });
}

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
            record_native_operation(
                InventoryNativeOperationKind::Da2StringRefill,
                Duration::from_nanos(1),
            );
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

struct ScriptedDa3Server {
    pages: Mutex<VecDeque<crate::backend::connector::NativeBrowsePage>>,
}

impl ConnectedServer for ScriptedDa3Server {
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
        _continuation: Option<&str>,
        _max_elements: u32,
        _filter: BrowseNodeFilter,
    ) -> OpcResult<crate::backend::connector::NativeBrowsePage> {
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| OpcError::Internal("scripted page queue exhausted".to_string()))
    }

    fn begin_da2_browse(
        &self,
        _browse_type: u32,
        _filter: Option<&str>,
        _data_type: u16,
        _access_rights: u32,
    ) -> OpcResult<Box<dyn BrowseStringIterator>> {
        Err(OpcError::NotImplemented("test".to_string()))
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

fn scripted_da3_page(
    names: &[&str],
    more_elements: bool,
    continuation: Option<&str>,
) -> crate::backend::connector::NativeBrowsePage {
    crate::backend::connector::NativeBrowsePage {
        elements: names
            .iter()
            .map(|name| NativeBrowseElement {
                name: (*name).to_string(),
                item_id: Some(format!("exact::{name}")),
                has_children: false,
                is_item: true,
            })
            .collect(),
        more_elements,
        continuation: continuation.map(str::to_string),
    }
}

fn run_scripted_da3_inventory(
    pages: Vec<crate::backend::connector::NativeBrowsePage>,
) -> (
    OpcResult<()>,
    Vec<InventoryEntry>,
    Option<InventoryCompleted>,
    Option<OpcError>,
) {
    let connector = SharedConnector {
        server: Arc::new(Mutex::new(Some(ScriptedDa3Server {
            pages: Mutex::new(pages.into()),
        }))),
    };
    let (sender, mut receiver) = mpsc::channel(512);
    let result = run_inventory(
        &connector,
        "test",
        InventoryOptions::default(),
        &InventoryControl::new(),
        &sender,
    );
    let (entries, completed, error) = collect(&mut receiver);
    (result, entries, completed, error)
}

#[test]
fn da3_inventory_accepts_a_temporary_empty_continuation_page() {
    let (result, entries, completed, error) = run_scripted_da3_inventory(vec![
        scripted_da3_page(&["First"], true, Some("first")),
        scripted_da3_page(&[], true, Some("empty")),
        scripted_da3_page(&["Last"], false, None),
    ]);

    assert!(result.is_ok());
    assert_eq!(
        entries
            .into_iter()
            .map(|entry| entry.item_id)
            .collect::<Vec<_>>(),
        vec!["exact::First", "exact::Last"]
    );
    assert!(completed.is_some_and(|value| value.complete));
    assert!(error.is_none());
}

#[test]
fn da3_inventory_rejects_missing_continuation_tokens() {
    let (result, _, completed, error) =
        run_scripted_da3_inventory(vec![scripted_da3_page(&["First"], true, None)]);

    assert!(matches!(
        result,
        Err(OpcError::BrowseContinuationNonProgress { detail, .. })
            if detail.contains("without a continuation point")
    ));
    assert!(completed.is_none());
    assert!(error.is_none());
}

#[test]
fn da3_inventory_rejects_empty_continuation_tokens() {
    let (result, _, completed, error) =
        run_scripted_da3_inventory(vec![scripted_da3_page(&["First"], true, Some(""))]);

    assert!(matches!(
        result,
        Err(OpcError::BrowseContinuationNonProgress { detail, .. })
            if detail.contains("empty continuation token")
    ));
    assert!(completed.is_none());
    assert!(error.is_none());
}

#[test]
fn da3_inventory_rejects_repeated_continuation_tokens() {
    let (result, _, completed, error) = run_scripted_da3_inventory(vec![
        scripted_da3_page(&["First"], true, Some("repeat")),
        scripted_da3_page(&["Second"], true, Some("repeat")),
    ]);

    assert!(matches!(
        result,
        Err(OpcError::BrowseContinuationNonProgress { detail, .. })
            if detail.contains("repeated continuation token")
    ));
    assert!(completed.is_none());
    assert!(error.is_none());
}

#[test]
fn da3_inventory_rejects_cyclic_continuation_tokens() {
    let (result, _, completed, error) = run_scripted_da3_inventory(vec![
        scripted_da3_page(&["First"], true, Some("a")),
        scripted_da3_page(&["Second"], true, Some("b")),
        scripted_da3_page(&["Third"], true, Some("a")),
    ]);

    assert!(matches!(
        result,
        Err(OpcError::BrowseContinuationNonProgress { detail, .. })
            if detail.contains("repeated continuation token")
    ));
    assert!(completed.is_none());
    assert!(error.is_none());
}

#[test]
fn da3_inventory_rejects_too_many_consecutive_empty_pages() {
    let mut pages = vec![scripted_da3_page(&["First"], true, Some("token-0"))];
    for index in 1..=MAX_CONSECUTIVE_EMPTY_DA3_PAGES {
        pages.push(scripted_da3_page(
            &[],
            true,
            Some(&format!("token-{index}")),
        ));
    }

    let (result, _, completed, error) = run_scripted_da3_inventory(pages);

    assert!(matches!(
        result,
        Err(OpcError::BrowseContinuationNonProgress { detail, .. })
            if detail.contains("consecutive empty pages")
    ));
    assert!(completed.is_none());
    assert!(error.is_none());
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
    assert_eq!(entries, Vec::<InventoryEntry>::new());
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
    assert_eq!(entries, Vec::<InventoryEntry>::new());
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
fn da3_subtree_inventory_starts_with_the_requested_root_breadcrumb() {
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(Da3Server {
            total: 1,
            browse_calls: Arc::new(AtomicUsize::new(0)),
            batch_sizes: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            da3_hresult: None,
            supports_da2: false,
            da2_items: Vec::new(),
        }))),
    });
    let (sender, mut receiver) = mpsc::channel(16);

    run_inventory_at_root(
        connector.as_ref(),
        "test",
        Some("FCS0207"),
        InventoryOptions {
            batch_size: 10,
            max_entries: None,
        },
        &InventoryControl::new(),
        &sender,
    )
    .unwrap();

    let (entries, completed, error) = collect(&mut receiver);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].breadcrumbs, vec!["FCS0207".to_string()]);
    assert!(completed.is_some_and(|value| value.complete));
    assert!(error.is_none());
}

#[test]
fn da3_subtree_compatibility_failure_is_terminal_without_da2_fallback() {
    for hresult in [RPC_X_NULL_REF_POINTER_HRESULT, E_NOTIMPL_HRESULT] {
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(Da3Server {
                total: 0,
                browse_calls: Arc::new(AtomicUsize::new(0)),
                batch_sizes: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                da3_hresult: Some(hresult),
                supports_da2: true,
                da2_items: vec!["must-not-fallback".to_string()],
            }))),
        });
        let (sender, mut receiver) = mpsc::channel(16);

        let result = run_inventory_at_root(
            connector.as_ref(),
            "test",
            Some("FCS0207"),
            InventoryOptions {
                batch_size: 10,
                max_entries: None,
            },
            &InventoryControl::new(),
            &sender,
        );

        assert!(matches!(
            result,
            Err(OpcError::Internal(message))
                if message.contains("browse_da3")
                    && message.contains("FCS0207")
        ));
        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(entries, Vec::<InventoryEntry>::new());
        assert!(completed.is_none());
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
    assert_eq!(
        boundary.before_operation_with_cost(1),
        BoundaryResult::Proceed
    );
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
    assert_eq!(
        boundary.before_operation_with_cost(1),
        BoundaryResult::Proceed
    );
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
    let _telemetry_scope = install_native_operation_telemetry(boundary.telemetry_recorder());
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
    let observations = boundary.take_operation_observations();
    assert_eq!(observations.len(), 1);
    assert_eq!(
        observations[0].kind,
        InventoryNativeOperationKind::Da2StringRefill
    );
    assert_eq!(observations[0].count, 2);
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
    assert_eq!(
        slices[0]
            .native_operation_observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![InventoryNativeOperationKind::Da3Page]
    );
    assert_eq!(slices[0].native_operation_observations[0].count, 1);
    assert_eq!(slices[1].sequence, 2);
}

#[test]
fn startup_telemetry_is_separate_from_the_first_slice() {
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(Da3Server {
            total: 1,
            browse_calls: Arc::new(AtomicUsize::new(0)),
            batch_sizes: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            da3_hresult: None,
            supports_da2: true,
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

    let mut completed = None;
    let mut slices = Vec::new();
    let mut error = None;
    while let Ok(event) = receiver.try_recv() {
        match event {
            Ok(InventoryEvent::Slice(slice)) => slices.push(slice),
            Ok(InventoryEvent::Completed(result)) => completed = Some(result),
            Ok(InventoryEvent::Entry(_) | InventoryEvent::Progress(_)) => {}
            Err(value) => error = Some(value),
        }
    }

    assert!(error.is_none());
    let completed = completed.expect("inventory should complete");
    assert_eq!(
        completed
            .startup_native_operation_observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![InventoryNativeOperationKind::NamespaceOrganizationQuery]
    );
    assert_eq!(completed.startup_native_operation_observations[0].count, 1);
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].native_operations, 1);
    assert_eq!(
        slices[0]
            .native_operation_observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![InventoryNativeOperationKind::Da3Page]
    );
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
fn da2_inventory_does_not_probe_item_children_or_branch_classification() {
    let server = Da2SemanticsServer::default();
    let organization_queries = Arc::clone(&server.organization_queries);
    let down_calls = Arc::clone(&server.down_calls);
    let up_calls = Arc::clone(&server.up_calls);
    let browse_to_calls = Arc::clone(&server.browse_to_calls);
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(server))),
    });
    let (sender, mut receiver) = mpsc::channel(16);
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

    let mut entries = Vec::new();
    let mut slices = Vec::new();
    let mut completed = None;
    let mut error = None;
    while let Ok(event) = receiver.try_recv() {
        match event {
            Ok(InventoryEvent::Entry(entry)) => entries.push(entry),
            Ok(InventoryEvent::Slice(slice)) => slices.push(slice),
            Ok(InventoryEvent::Completed(result)) => completed = Some(result),
            Ok(InventoryEvent::Progress(_)) => {}
            Err(value) => error = Some(value),
        }
    }
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item_id.as_str())
            .collect::<Vec<_>>(),
        vec!["Pump", "Pressure", "Pump.PV"]
    );
    assert_eq!(organization_queries.load(Ordering::Relaxed), 1);
    assert_eq!(browse_to_calls.load(Ordering::Relaxed), 1);
    assert_eq!(down_calls.load(Ordering::Relaxed), 0);
    assert_eq!(up_calls.load(Ordering::Relaxed), 0);
    let observations = slices
        .iter()
        .flat_map(|slice| slice.native_operation_observations.iter())
        .collect::<Vec<_>>();
    assert!(
        !observations
            .iter()
            .any(|observation| observation.kind == InventoryNativeOperationKind::Da2PathDown)
    );
    assert!(
        observations
            .iter()
            .any(|observation| observation.kind == InventoryNativeOperationKind::Da2PathTo)
    );
    assert!(!observations.iter().any(|observation| {
        matches!(
            observation.kind,
            InventoryNativeOperationKind::Da2ClassificationDown
                | InventoryNativeOperationKind::Da2ClassificationUp
                | InventoryNativeOperationKind::Da2ProbeDown
                | InventoryNativeOperationKind::Da2ProbeUp
        )
    }));
    assert!(completed.is_some_and(|value| value.complete));
    assert!(error.is_none());
}

#[test]
fn da2_browse_to_fallback_preserves_position_for_expected_rejections() {
    for behavior in [
        Da2BrowseToBehavior::Unsupported,
        Da2BrowseToBehavior::InvalidArgument,
    ] {
        let server = Da2SemanticsServer {
            browse_to_behavior: behavior,
            ..Default::default()
        };
        let browse_to_calls = Arc::clone(&server.browse_to_calls);
        let down_calls = Arc::clone(&server.down_calls);
        let connector = Arc::new(SharedConnector {
            server: Arc::new(Mutex::new(Some(server))),
        });
        let (sender, mut receiver) = mpsc::channel(32);

        assert!(
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
            .is_ok()
        );

        let (entries, completed, error) = collect(&mut receiver);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["Pump", "Pressure", "Pump.PV"]
        );
        assert!(completed.is_some_and(|value| value.complete));
        assert!(error.is_none());
        assert_eq!(browse_to_calls.load(Ordering::Relaxed), 1);
        assert_eq!(down_calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn da2_browse_to_unexpected_error_is_terminal_without_fallback() {
    let server = Da2SemanticsServer {
        browse_to_behavior: Da2BrowseToBehavior::Fatal,
        ..Default::default()
    };
    let browse_to_calls = Arc::clone(&server.browse_to_calls);
    let down_calls = Arc::clone(&server.down_calls);
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(server))),
    });
    let (sender, mut receiver) = mpsc::channel(32);

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
    assert!(result.is_err_and(|value| { value.to_string().contains("change_browse_position_to") }));

    let (entries, completed, error) = collect(&mut receiver);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item_id.as_str())
            .collect::<Vec<_>>(),
        vec!["Pump", "Pressure"]
    );
    assert!(completed.is_none());
    assert!(error.is_none());
    assert_eq!(browse_to_calls.load(Ordering::Relaxed), 1);
    assert_eq!(down_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn da2_browse_to_cancellation_keeps_terminal_lifecycle() {
    let control = InventoryControl::new();
    let server = Da2SemanticsServer {
        cancel_on_first_browse_to: Mutex::new(Some(control.clone())),
        ..Default::default()
    };
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(server))),
    });
    let (sender, mut receiver) = mpsc::channel(32);

    run_inventory(
        connector.as_ref(),
        "test",
        InventoryOptions {
            batch_size: 10,
            max_entries: None,
        },
        &control,
        &sender,
    )
    .unwrap();

    let (entries, completed, error) = collect(&mut receiver);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item_id.as_str())
            .collect::<Vec<_>>(),
        vec!["Pump", "Pressure"]
    );
    assert!(completed.is_some_and(|value| value.cancelled && !value.complete));
    assert!(error.is_none());
}

#[test]
fn da2_deferred_branch_open_can_be_cancelled_without_losing_emitted_item() {
    let control = InventoryControl::new();
    let server = Da2SemanticsServer {
        browse_to_behavior: Da2BrowseToBehavior::Unsupported,
        cancel_on_first_down: Mutex::new(Some(control.clone())),
        ..Default::default()
    };
    let connector = Arc::new(SharedConnector {
        server: Arc::new(Mutex::new(Some(server))),
    });
    let (sender, mut receiver) = mpsc::channel(16);

    run_inventory(
        connector.as_ref(),
        "test",
        InventoryOptions {
            batch_size: 10,
            max_entries: None,
        },
        &control,
        &sender,
    )
    .unwrap();

    let (entries, completed, error) = collect(&mut receiver);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item_id.as_str())
            .collect::<Vec<_>>(),
        vec!["Pump", "Pressure"]
    );
    assert!(completed.is_some_and(|value| value.cancelled && !value.complete));
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
    assert_eq!(entries[0].kind, BrowseNodeKind::BranchAndItem);
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
    let (sender, mut receiver) = mpsc::channel(256);
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
            if message.contains("change_browse_position(down)")
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
        parent_item_id: None,
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
    organization_queries: Arc<AtomicUsize>,
    down_calls: Arc<AtomicUsize>,
    up_calls: Arc<AtomicUsize>,
    browse_to_calls: Arc<AtomicUsize>,
    browse_to_behavior: Da2BrowseToBehavior,
    cancel_on_first_down: Mutex<Option<InventoryControl>>,
    cancel_on_first_browse_to: Mutex<Option<InventoryControl>>,
}

#[derive(Clone, Copy, Default)]
enum Da2BrowseToBehavior {
    #[default]
    Succeed,
    Unsupported,
    InvalidArgument,
    Fatal,
}

impl ConnectedServer for Da2SemanticsServer {
    type Group = TestGroup;

    fn query_organization(&self) -> OpcResult<u32> {
        self.organization_queries.fetch_add(1, Ordering::Relaxed);
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
            self.down_calls.fetch_add(1, Ordering::Relaxed);
            position.push(name.to_string());
        } else if direction == OPC_BROWSE_UP.0.cast_unsigned() {
            self.up_calls.fetch_add(1, Ordering::Relaxed);
            position.pop();
        }
        drop(position);
        let cancel_on_down = self.cancel_on_first_down.lock().unwrap().take();
        if direction == OPC_BROWSE_DOWN.0.cast_unsigned()
            && let Some(control) = cancel_on_down
        {
            control.cancel_with_reason("test_deferred_branch_open");
        }
        Ok(())
    }

    fn change_browse_position_to(&self, item_id: &str) -> OpcResult<()> {
        self.browse_to_calls.fetch_add(1, Ordering::Relaxed);
        let cancel_on_browse_to = self.cancel_on_first_browse_to.lock().unwrap().take();
        if let Some(control) = cancel_on_browse_to {
            control.cancel_with_reason("test_browse_to");
        }
        match self.browse_to_behavior {
            Da2BrowseToBehavior::Succeed => {
                if item_id == "Pump" {
                    self.position.lock().unwrap().push("Pump".to_string());
                    Ok(())
                } else {
                    Err(OpcError::InvalidState("unknown direct target".to_string()))
                }
            }
            Da2BrowseToBehavior::Unsupported => Err(OpcError::NotImplemented("test".to_string())),
            Da2BrowseToBehavior::InvalidArgument => Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(
                    E_INVALIDARG_HRESULT.cast_signed(),
                )),
            }),
            Da2BrowseToBehavior::Fatal => Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(0x8007_0005_u32.cast_signed())),
            }),
        }
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

    fn change_browse_position_to(&self, item_id: &str) -> OpcResult<()> {
        match item_id {
            "FCS0528.LeafOnly" => {
                let mut position = self.position.lock().unwrap();
                *position = vec!["FCS0528".to_string(), "LeafOnly".to_string()];
                drop(position);
                Ok(())
            }
            _ => Err(OpcError::InvalidState("unknown direct target".to_string())),
        }
    }

    fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
        let position = self.position.lock().unwrap();
        match (position.as_slice(), item_name) {
            ([], "FCS0528") => Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(0xC004_0007_u32.cast_signed())),
            }),
            ([area], "PV") if area == "FCS0528" => Ok("FCS0528.PV".to_string()),
            ([area], "LeafOnly") if area == "FCS0528" => Ok("FCS0528.LeafOnly".to_string()),
            ([area], "\u{1}" | "Denied") if area == "FCS0528" => Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(0xC004_0007_u32.cast_signed())),
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

fn run_inventory<C: ServerConnector>(
    connector: &C,
    server_name: &str,
    options: InventoryOptions,
    control: &InventoryControl,
    sender: &mpsc::Sender<OpcResult<InventoryEvent>>,
) -> OpcResult<()> {
    run_inventory_at_root(connector, server_name, None, options, control, sender)
}
