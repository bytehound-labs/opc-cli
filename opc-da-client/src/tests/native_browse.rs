//! Native session, token, mapping, and cancellation characterization.

use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, NativeBrowseElement, NativeBrowsePage,
    classify_da2_branch,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT, OPC_NS_HIERARCHIAL,
};
use crate::errors::{OpcError, OpcResult};
use crate::native_browse::BrowseSessions;
use crate::native_browse::capabilities::capabilities_for_server;
use crate::native_browse::da2::{BufferedBrowseIterator, Da2PageState};
use crate::native_browse::state::BrowseBackend;
use crate::provider::{
    BrowseNamespace, BrowseNode, BrowseNodeFilter, BrowseNodeKind, BrowseNodeToken,
    BrowsePageRequest, BrowsePageToken, BrowseSessionToken,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::backend::connector::{ConnectedGroup, RemoteArray};
use crate::bindings::da::{tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE};
use crate::opc_da::errors::{
    E_INVALIDARG_HRESULT, E_NOTIMPL_HRESULT, MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES,
    RPC_X_NULL_REF_POINTER_HRESULT,
};
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::System::Variant::VARIANT;
use windows::core::HRESULT;

type Da3Call = (Option<String>, Option<String>, BrowseNodeFilter);

#[test]
fn native_browse_events_keep_the_module_target() {
    crate::tests::tracing::assert_event_targets("opc_da_client::native_browse", || {
        let server = MockServer::da2(
            BrowseNamespace::Flat,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
        );
        let capabilities = capabilities_for_server(&server).unwrap();
        assert_eq!(capabilities.namespace, BrowseNamespace::Flat);
    });
}

struct MockGroup;

impl ConnectedGroup for MockGroup {
    fn add_items(
        &self,
        _items: &[tagOPCITEMDEF],
    ) -> OpcResult<(RemoteArray<tagOPCITEMRESULT>, RemoteArray<HRESULT>)> {
        Err(OpcError::NotImplemented("mock".to_string()))
    }

    fn read(
        &self,
        _source: tagOPCDATASOURCE,
        _server_handles: &[ItemHandle],
    ) -> OpcResult<(RemoteArray<tagOPCITEMSTATE>, RemoteArray<HRESULT>)> {
        Err(OpcError::NotImplemented("mock".to_string()))
    }

    fn write(
        &self,
        _server_handles: &[ItemHandle],
        _values: &[VARIANT],
    ) -> OpcResult<RemoteArray<HRESULT>> {
        Err(OpcError::NotImplemented("mock".to_string()))
    }
}

#[derive(Clone)]
struct MockServer {
    namespace: BrowseNamespace,
    da3: bool,
    da2: bool,
    da3_error: Option<u32>,
    da3_pages: Arc<Mutex<VecDeque<NativeBrowsePage>>>,
    da3_calls: Arc<Mutex<Vec<Da3Call>>>,
    position: Arc<Mutex<Vec<String>>>,
    branches: Arc<HashMap<Vec<String>, Vec<String>>>,
    items: Arc<HashMap<Vec<String>, Vec<String>>>,
    flat_items: Arc<Vec<String>>,
    invalidarg_branches: Arc<HashSet<String>>,
    non_navigable_branches: Arc<HashSet<String>>,
    non_progressing_branches: Arc<HashSet<Vec<String>>>,
    non_progressing_items: Arc<HashSet<Vec<String>>>,
    navigation_errors: Arc<HashMap<String, u32>>,
    drop_count: Option<Arc<AtomicUsize>>,
}

impl MockServer {
    fn da3(pages: Vec<NativeBrowsePage>) -> Self {
        Self {
            namespace: BrowseNamespace::Hierarchical,
            da3: true,
            da2: false,
            da3_error: None,
            da3_pages: Arc::new(Mutex::new(pages.into())),
            da3_calls: Arc::default(),
            position: Arc::default(),
            branches: Arc::default(),
            items: Arc::default(),
            flat_items: Arc::default(),
            invalidarg_branches: Arc::default(),
            non_navigable_branches: Arc::default(),
            non_progressing_branches: Arc::default(),
            non_progressing_items: Arc::default(),
            navigation_errors: Arc::default(),
            drop_count: None,
        }
    }

    fn da2(
        namespace: BrowseNamespace,
        branches: HashMap<Vec<String>, Vec<String>>,
        items: HashMap<Vec<String>, Vec<String>>,
        flat_items: Vec<String>,
    ) -> Self {
        Self {
            namespace,
            da3: false,
            da2: true,
            da3_error: None,
            da3_pages: Arc::default(),
            da3_calls: Arc::default(),
            position: Arc::default(),
            branches: Arc::new(branches),
            items: Arc::new(items),
            flat_items: Arc::new(flat_items),
            invalidarg_branches: Arc::default(),
            non_navigable_branches: Arc::default(),
            non_progressing_branches: Arc::default(),
            non_progressing_items: Arc::default(),
            navigation_errors: Arc::default(),
            drop_count: None,
        }
    }

    fn with_da2_fallback(mut self, hresult: u32, flat_items: Vec<String>) -> Self {
        self.namespace = BrowseNamespace::Flat;
        self.da2 = true;
        self.da3_error = Some(hresult);
        self.flat_items = Arc::new(flat_items);
        self
    }

    fn with_invalidarg_branch(mut self, name: &str, navigable: bool) -> Self {
        let mut invalidarg_branches = (*self.invalidarg_branches).clone();
        invalidarg_branches.insert(name.to_string());
        self.invalidarg_branches = Arc::new(invalidarg_branches);
        if !navigable {
            let mut non_navigable_branches = (*self.non_navigable_branches).clone();
            non_navigable_branches.insert(name.to_string());
            self.non_navigable_branches = Arc::new(non_navigable_branches);
        }
        self
    }

    fn with_non_navigable_branch(mut self, name: &str) -> Self {
        let mut non_navigable_branches = (*self.non_navigable_branches).clone();
        non_navigable_branches.insert(name.to_string());
        self.non_navigable_branches = Arc::new(non_navigable_branches);
        self
    }

    fn with_non_progressing_branch(mut self, path: &[&str]) -> Self {
        let mut paths = (*self.non_progressing_branches).clone();
        paths.insert(path.iter().map(|part| (*part).to_string()).collect());
        self.non_progressing_branches = Arc::new(paths);
        self
    }

    fn with_non_progressing_item(mut self, path: &[&str]) -> Self {
        let mut paths = (*self.non_progressing_items).clone();
        paths.insert(path.iter().map(|part| (*part).to_string()).collect());
        self.non_progressing_items = Arc::new(paths);
        self
    }

    fn with_navigation_error(mut self, name: &str, hresult: u32) -> Self {
        let mut navigation_errors = (*self.navigation_errors).clone();
        navigation_errors.insert(name.to_string(), hresult);
        self.navigation_errors = Arc::new(navigation_errors);
        self
    }

    fn with_drop_count(mut self, drop_count: Arc<AtomicUsize>) -> Self {
        self.drop_count = Some(drop_count);
        self
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(drop_count) = &self.drop_count {
            drop_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl ConnectedServer for MockServer {
    type Group = MockGroup;

    fn query_organization(&self) -> OpcResult<u32> {
        Ok(match self.namespace {
            BrowseNamespace::Flat => OPC_NS_FLAT.0.cast_unsigned(),
            BrowseNamespace::Hierarchical | BrowseNamespace::Unknown => {
                OPC_NS_HIERARCHIAL.0.cast_unsigned()
            }
        })
    }

    fn browse_opc_item_ids(
        &self,
        _browse_type: u32,
        _filter: Option<&str>,
        _data_type: u16,
        _access_rights: u32,
    ) -> OpcResult<crate::backend::connector::StringIterator> {
        Err(OpcError::NotImplemented("mock".to_string()))
    }

    fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
        if direction == OPC_BROWSE_DOWN.0.cast_unsigned() {
            let position = self.position.lock().unwrap().clone();
            if name == "\u{1}" && self.non_progressing_branches.contains(&position) {
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(
                        E_INVALIDARG_HRESULT.cast_signed(),
                    )),
                });
            }
            if let Some(hresult) = self.navigation_errors.get(name) {
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT((*hresult).cast_signed())),
                });
            }
            if self.non_navigable_branches.contains(name) {
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(HRESULT(
                        E_INVALIDARG_HRESULT.cast_signed(),
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
        if !self
            .items
            .get(position.as_slice())
            .is_some_and(|items| items.iter().any(|item| item == item_name))
        {
            return Err(OpcError::InvalidState(format!(
                "'{item_name}' is not an item at this browse position"
            )));
        }
        let prefix = if position.is_empty() {
            String::new()
        } else {
            format!("{}.", position.join("."))
        };
        drop(position);
        Ok(format!("exact::{prefix}{item_name}"))
    }

    fn resolve_da2_item_id(&self, item_name: &str) -> OpcResult<Option<String>> {
        if self.invalidarg_branches.contains(item_name) {
            return Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(
                    E_INVALIDARG_HRESULT.cast_signed(),
                )),
            });
        }
        let position = self.position.lock().unwrap();
        let is_item = self
            .items
            .get(position.as_slice())
            .is_some_and(|items| items.iter().any(|item| item == item_name));
        let prefix = if position.is_empty() {
            String::new()
        } else {
            format!("{}.", position.join("."))
        };
        drop(position);
        Ok(is_item.then(|| format!("exact::{prefix}{item_name}")))
    }

    fn da2_name_has_children(&self, item_name: &str) -> OpcResult<bool> {
        if self.non_navigable_branches.contains(item_name) {
            return Ok(false);
        }
        let position = self.position.lock().unwrap();
        let is_branch = self
            .branches
            .get(position.as_slice())
            .is_some_and(|branches| branches.iter().any(|branch| branch == item_name));
        drop(position);
        Ok(is_branch)
    }

    fn supports_da2_browse(&self) -> bool {
        self.da2
    }

    fn supports_da3_browse(&self) -> bool {
        self.da3
    }

    fn begin_da2_browse(
        &self,
        browse_type: u32,
        _filter: Option<&str>,
        _data_type: u16,
        _access_rights: u32,
    ) -> OpcResult<Box<dyn BrowseStringIterator>> {
        let position = self.position.lock().unwrap().clone();
        if browse_type == OPC_BRANCH.0.cast_unsigned()
            && self.non_progressing_branches.contains(&position)
        {
            return Ok(Box::new(std::iter::repeat_with(|| {
                Ok::<String, OpcError>("\u{1}".to_string())
            })));
        }
        if browse_type == OPC_LEAF.0.cast_unsigned()
            && self.non_progressing_items.contains(&position)
        {
            let item = self
                .items
                .get(&position)
                .and_then(|items| items.first())
                .cloned()
                .unwrap_or_else(|| "RepeatedItem".to_string());
            return Ok(Box::new(std::iter::repeat_with(move || {
                Ok::<String, OpcError>(item.clone())
            })));
        }
        let values = if browse_type == OPC_BRANCH.0.cast_unsigned() {
            self.branches.get(&position).cloned().unwrap_or_default()
        } else if browse_type == OPC_LEAF.0.cast_unsigned() {
            self.items.get(&position).cloned().unwrap_or_default()
        } else if browse_type == OPC_FLAT.0.cast_unsigned() {
            self.flat_items.as_ref().clone()
        } else {
            return Err(OpcError::InvalidState("unexpected browse type".to_string()));
        };
        Ok(Box::new(values.into_iter().map(Ok)))
    }

    fn browse_da3(
        &self,
        item_id: Option<&str>,
        continuation: Option<&str>,
        _max_elements: u32,
        filter: BrowseNodeFilter,
    ) -> OpcResult<NativeBrowsePage> {
        self.da3_calls.lock().unwrap().push((
            item_id.map(str::to_string),
            continuation.map(str::to_string),
            filter,
        ));
        if let Some(hresult) = self.da3_error {
            return Err(OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(hresult.cast_signed())),
            });
        }
        self.da3_pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| OpcError::Internal("missing mock DA3 page".to_string()))
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
        Err(OpcError::NotImplemented("mock".to_string()))
    }

    fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
        Err(OpcError::NotImplemented("mock".to_string()))
    }
}

fn request(
    parent: Option<BrowseNodeToken>,
    filter: BrowseNodeFilter,
    max_elements: u32,
    continuation: Option<BrowsePageToken>,
) -> BrowsePageRequest {
    BrowsePageRequest {
        parent,
        filter,
        max_elements,
        continuation,
    }
}

#[test]
fn da3_root_compatibility_failures_fall_back_to_da2_for_the_session() {
    for hresult in [RPC_X_NULL_REF_POINTER_HRESULT, E_NOTIMPL_HRESULT] {
        let server = MockServer::da3(Vec::new())
            .with_da2_fallback(hresult, vec!["Channel.Device.Tag".to_string()]);
        let calls = Arc::clone(&server.da3_calls);
        let mut sessions = BrowseSessions::default();
        let session = sessions.open(server).unwrap();

        let first = sessions
            .page(&session, request(None, BrowseNodeFilter::All, 10, None))
            .unwrap();
        assert_eq!(first.nodes.len(), 1);
        assert_eq!(first.nodes[0].name, "Channel.Device.Tag");
        assert_eq!(
            first.nodes[0].item_id.as_deref(),
            Some("Channel.Device.Tag")
        );
        assert_eq!(first.nodes[0].kind, BrowseNodeKind::Item);

        let second = sessions
            .page(&session, request(None, BrowseNodeFilter::All, 10, None))
            .unwrap();
        assert_eq!(second.nodes.len(), 1);
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}

#[test]
fn da3_root_operational_failure_does_not_fall_back() {
    let server = MockServer::da3(Vec::new())
        .with_da2_fallback(0x8007_0005, vec!["must-not-be-returned".to_string()]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    assert!(matches!(
        sessions.page(&session, request(None, BrowseNodeFilter::All, 10, None)),
        Err(OpcError::Com { source })
            if source.code().0.cast_unsigned() == 0x8007_0005
    ));
}

#[test]
fn da3_root_compatibility_failure_after_success_does_not_change_backend() {
    let mut server = MockServer::da3(vec![NativeBrowsePage {
        elements: Vec::new(),
        more_elements: false,
        continuation: None,
    }]);
    server.namespace = BrowseNamespace::Flat;
    server.da2 = true;
    server.flat_items = Arc::new(vec!["must-not-be-returned".to_string()]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    sessions
        .page(&session, request(None, BrowseNodeFilter::All, 10, None))
        .unwrap();
    sessions
        .sessions
        .get_mut(&session)
        .unwrap()
        .server
        .da3_error = Some(RPC_X_NULL_REF_POINTER_HRESULT);

    assert!(matches!(
        sessions.page(&session, request(None, BrowseNodeFilter::All, 10, None)),
        Err(OpcError::Com { source })
            if source.code().0.cast_unsigned() == RPC_X_NULL_REF_POINTER_HRESULT
    ));
    let state = sessions.sessions.get(&session).unwrap();
    assert!(state.da3_root_succeeded);
    assert!(state.capabilities.supports_da3);
    assert!(matches!(state.backend, BrowseBackend::Da3));
}

#[test]
#[allow(clippy::too_many_lines)]
fn da3_maps_node_kinds_and_hides_continuations() {
    let server = MockServer::da3(vec![
        NativeBrowsePage {
            elements: vec![
                NativeBrowseElement {
                    name: "Branch".to_string(),
                    item_id: Some("raw.branch".to_string()),
                    has_children: true,
                    is_item: false,
                },
                NativeBrowseElement {
                    name: "Item".to_string(),
                    item_id: Some("raw.item".to_string()),
                    has_children: false,
                    is_item: true,
                },
                NativeBrowseElement {
                    name: "Both".to_string(),
                    item_id: Some("raw.both".to_string()),
                    has_children: true,
                    is_item: true,
                },
            ],
            more_elements: true,
            continuation: Some("raw-da3-continuation".to_string()),
        },
        NativeBrowsePage {
            elements: vec![],
            more_elements: false,
            continuation: None,
        },
        NativeBrowsePage {
            elements: vec![],
            more_elements: false,
            continuation: None,
        },
    ]);
    let calls = server.da3_calls.clone();
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();
    assert_eq!(
        BrowseSessionToken::parse(&session.to_string()).unwrap(),
        session
    );

    let first = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 3, None))
        .unwrap();
    assert_eq!(
        first.nodes.iter().map(|node| node.kind).collect::<Vec<_>>(),
        vec![
            BrowseNodeKind::Branch,
            BrowseNodeKind::Item,
            BrowseNodeKind::BranchAndItem
        ]
    );
    assert_eq!(first.nodes[0].item_id, None);
    assert_eq!(first.nodes[1].item_id.as_deref(), Some("raw.item"));
    assert_eq!(first.nodes[2].item_id.as_deref(), Some("raw.both"));
    let continuation = first.continuation.unwrap();
    assert_ne!(continuation.to_string(), "raw-da3-continuation");
    assert_eq!(
        BrowsePageToken::parse(&continuation.to_string()).unwrap(),
        continuation
    );
    assert_eq!(
        BrowseNodeToken::parse(&first.nodes[0].token.to_string()).unwrap(),
        first.nodes[0].token
    );
    assert_eq!(
        uuid::Uuid::parse_str(&continuation.to_string())
            .unwrap()
            .get_version(),
        Some(uuid::Version::Random)
    );
    let branch = first.nodes[0].token;

    let second = sessions
        .page(
            &session,
            request(None, BrowseNodeFilter::All, 3, Some(continuation)),
        )
        .unwrap();
    assert_eq!(second.nodes, Vec::<BrowseNode>::new());
    assert!(second.continuation.is_none());
    assert_eq!(
        calls.lock().unwrap()[1],
        (
            None,
            Some("raw-da3-continuation".to_string()),
            BrowseNodeFilter::All
        )
    );

    sessions
        .page(
            &session,
            request(Some(branch), BrowseNodeFilter::Branches, 3, None),
        )
        .unwrap();
    assert_eq!(
        calls.lock().unwrap()[2],
        (
            Some("raw.branch".to_string()),
            None,
            BrowseNodeFilter::Branches
        )
    );
}

#[test]
fn da3_rejects_selectable_nodes_without_exact_item_ids() {
    let server = MockServer::da3(vec![NativeBrowsePage {
        elements: vec![NativeBrowseElement {
            name: "MissingItemId".to_string(),
            item_id: None,
            has_children: false,
            is_item: true,
        }],
        more_elements: false,
        continuation: None,
    }]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();
    let result = sessions.page(&session, request(None, BrowseNodeFilter::All, 1, None));
    assert!(matches!(
        result,
        Err(OpcError::Internal(message)) if message.contains("did not include an item ID")
    ));
}

#[test]
fn da2_returns_only_immediate_branches_and_exact_item_ids() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["Area".to_string()]);
    branches.insert(vec!["Area".to_string()], vec!["Nested".to_string()]);
    let mut items = HashMap::new();
    items.insert(Vec::new(), vec!["RootTag".to_string()]);
    items.insert(vec!["Area".to_string()], vec!["AreaTag".to_string()]);
    let server = MockServer::da2(BrowseNamespace::Hierarchical, branches, items, vec![]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let root = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 10, None))
        .unwrap();
    assert_eq!(root.nodes.len(), 2);
    assert_eq!(root.nodes[0].name, "Area");
    assert_eq!(root.nodes[0].kind, BrowseNodeKind::Branch);
    assert_eq!(root.nodes[1].item_id.as_deref(), Some("exact::RootTag"));
    assert!(root.nodes.iter().all(|node| node.name != "Nested"));

    let area = root.nodes[0].token;
    let children = sessions
        .page(
            &session,
            request(Some(area), BrowseNodeFilter::Items, 10, None),
        )
        .unwrap();
    assert_eq!(children.nodes.len(), 1);
    assert_eq!(
        children.nodes[0].item_id.as_deref(),
        Some("exact::Area.AreaTag")
    );
}

#[test]
fn da2_recovers_non_progressing_branch_and_preserves_items_and_pagination() {
    let mut items = HashMap::new();
    items.insert(
        Vec::new(),
        vec!["FirstItem".to_string(), "SecondItem".to_string()],
    );
    let server = MockServer::da2(BrowseNamespace::Hierarchical, HashMap::new(), items, vec![])
        .with_non_progressing_branch(&[]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let first = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 1, None))
        .unwrap();
    assert_eq!(first.nodes[0].name, "FirstItem");
    let second = sessions
        .page(
            &session,
            request(None, BrowseNodeFilter::All, 1, first.continuation),
        )
        .unwrap();
    assert_eq!(second.nodes[0].name, "SecondItem");
    assert!(second.continuation.is_none());
}

#[test]
fn da2_recovers_non_progressing_branch_for_branch_filter() {
    let server = MockServer::da2(
        BrowseNamespace::Hierarchical,
        HashMap::new(),
        HashMap::new(),
        vec![],
    )
    .with_non_progressing_branch(&[]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let page = sessions
        .page(
            &session,
            request(None, BrowseNodeFilter::Branches, 10, None),
        )
        .unwrap();
    assert_eq!(page.nodes, Vec::<BrowseNode>::new());
    assert!(page.continuation.is_none());
}

#[test]
fn da2_item_iterator_non_progress_remains_fatal() {
    let mut items = HashMap::new();
    items.insert(Vec::new(), vec!["RepeatedItem".to_string()]);
    let server = MockServer::da2(BrowseNamespace::Hierarchical, HashMap::new(), items, vec![])
        .with_non_progressing_item(&[]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    assert!(matches!(
        sessions.page(
            &session,
            request(
                None,
                BrowseNodeFilter::Items,
                u32::try_from(MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES)
                    .expect("the non-progress guard limit fits in u32"),
                None,
            ),
        ),
        Err(OpcError::BrowseNonProgress { iterator_type, .. })
            if iterator_type == "native DA2 item iterator"
    ));
}

#[test]
fn da2_has_more_discards_prefetched_branch_non_progress_and_uses_items() {
    let mut state = Da2PageState {
        parent_path: vec!["SCS0130".to_string()],
        branches: Some(BufferedBrowseIterator::new(
            Box::new(std::iter::repeat_with(|| {
                Ok::<String, OpcError>("\u{1}".to_string())
            })),
            "native DA2 branch iterator",
            &["SCS0130".to_string()],
        )),
        items: Some(BufferedBrowseIterator::new(
            Box::new(std::iter::once(Ok::<String, OpcError>("PV".to_string()))),
            "native DA2 item iterator",
            &["SCS0130".to_string()],
        )),
        flat: false,
        merged_items: HashSet::new(),
    };
    for _ in 0..63 {
        assert!(matches!(
            state.branches.as_mut().unwrap().next(),
            Some(Ok(value)) if value == "\u{1}"
        ));
    }

    assert!(state.has_more());
    assert!(state.branches.is_none());
    assert_eq!(
        state.next().unwrap(),
        Some((BrowseNodeKind::Item, "PV".to_string()))
    );
}

#[test]
fn da2_skips_branch_only_navigation_rejections_but_keeps_navigable_ones() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["Bad".to_string(), "Odd".to_string()]);
    branches.insert(vec!["Odd".to_string()], Vec::new());
    let mut items = HashMap::new();
    items.insert(vec!["Odd".to_string()], vec!["PV".to_string()]);
    let server = MockServer::da2(BrowseNamespace::Hierarchical, branches, items, vec![])
        .with_non_navigable_branch("Bad")
        .with_invalidarg_branch("Odd", true);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let root = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 1, None))
        .unwrap();
    assert_eq!(root.nodes.len(), 1);
    assert_eq!(root.nodes[0].name, "Odd");
    assert_eq!(root.nodes[0].kind, BrowseNodeKind::Branch);

    let children = sessions
        .page(
            &session,
            request(Some(root.nodes[0].token), BrowseNodeFilter::Items, 10, None),
        )
        .unwrap();
    assert_eq!(children.nodes.len(), 1);
    assert_eq!(children.nodes[0].item_id.as_deref(), Some("exact::Odd.PV"));
}

#[test]
fn da2_preserves_non_navigable_branch_entries_that_are_exact_items() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["LeafOnly".to_string()]);
    let mut items = HashMap::new();
    items.insert(Vec::new(), vec!["LeafOnly".to_string()]);
    let server = MockServer::da2(
        BrowseNamespace::Hierarchical,
        branches.clone(),
        items.clone(),
        vec![],
    )
    .with_non_navigable_branch("LeafOnly");
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let root = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 10, None))
        .unwrap();
    assert_eq!(root.nodes.len(), 1);
    assert_eq!(root.nodes[0].name, "LeafOnly");
    assert_eq!(root.nodes[0].kind, BrowseNodeKind::Item);
    assert_eq!(root.nodes[0].item_id.as_deref(), Some("exact::LeafOnly"));

    let branches_only_server =
        MockServer::da2(BrowseNamespace::Hierarchical, branches, items, vec![])
            .with_non_navigable_branch("LeafOnly");
    let branches_only_session = sessions.open(branches_only_server).unwrap();
    let branches_only = sessions
        .page(
            &branches_only_session,
            request(None, BrowseNodeFilter::Branches, 10, None),
        )
        .unwrap();
    assert_eq!(branches_only.nodes, Vec::<BrowseNode>::new());
}

#[test]
fn da2_branch_navigation_propagates_non_invalidarg_errors() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["Denied".to_string()]);
    let server = MockServer::da2(
        BrowseNamespace::Hierarchical,
        branches,
        HashMap::new(),
        vec![],
    )
    .with_navigation_error("Denied", 0x8007_0005);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    assert!(matches!(
        sessions.page(&session, request(None, BrowseNodeFilter::All, 10, None)),
        Err(OpcError::Internal(message))
            if message.contains("classify_da2_branch")
                && message.contains("\"Denied\"")
                && message.contains("0x80070005")
    ));
}

#[test]
fn da2_merges_same_named_branch_and_leaf_across_pages() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["Pump".to_string()]);
    let mut items = HashMap::new();
    items.insert(Vec::new(), vec!["Pump".to_string(), "Pressure".to_string()]);
    let server = MockServer::da2(BrowseNamespace::Hierarchical, branches, items, vec![]);
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let first = sessions
        .page(&session, request(None, BrowseNodeFilter::All, 1, None))
        .unwrap();
    assert_eq!(first.nodes.len(), 1);
    assert_eq!(first.nodes[0].name, "Pump");
    assert_eq!(first.nodes[0].kind, BrowseNodeKind::BranchAndItem);
    assert_eq!(first.nodes[0].item_id.as_deref(), Some("exact::Pump"));

    let second = sessions
        .page(
            &session,
            request(None, BrowseNodeFilter::All, 2, first.continuation),
        )
        .unwrap();
    assert_eq!(second.nodes.len(), 1);
    assert_eq!(second.nodes[0].name, "Pressure");
    assert_eq!(second.nodes[0].item_id.as_deref(), Some("exact::Pressure"));
    assert!(second.continuation.is_none());

    let items_only = sessions
        .page(&session, request(None, BrowseNodeFilter::Items, 2, None))
        .unwrap();
    assert_eq!(items_only.nodes[0].name, "Pump");
    assert_eq!(items_only.nodes[0].kind, BrowseNodeKind::BranchAndItem);
    assert_eq!(items_only.nodes[0].item_id.as_deref(), Some("exact::Pump"));
}

#[test]
fn flat_namespace_pages_without_recursion() {
    let server = MockServer::da2(
        BrowseNamespace::Flat,
        HashMap::new(),
        HashMap::new(),
        vec!["A".to_string(), "B".to_string(), "C".to_string()],
    );
    let mut sessions = BrowseSessions::default();
    let session = sessions.open(server).unwrap();

    let first = sessions
        .page(&session, request(None, BrowseNodeFilter::Items, 2, None))
        .unwrap();
    assert_eq!(
        first
            .nodes
            .iter()
            .map(|node| node.item_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("A"), Some("B")]
    );

    let second = sessions
        .page(
            &session,
            request(None, BrowseNodeFilter::Items, 2, first.continuation),
        )
        .unwrap();
    assert_eq!(second.nodes[0].item_id.as_deref(), Some("C"));
    assert!(second.continuation.is_none());
}

#[test]
fn da2_sessions_keep_independent_browse_positions() {
    let mut branches = HashMap::new();
    branches.insert(Vec::new(), vec!["Area".to_string()]);
    let mut items = HashMap::new();
    items.insert(Vec::new(), vec!["Root".to_string()]);
    items.insert(vec!["Area".to_string()], vec!["Child".to_string()]);
    let first_server = MockServer::da2(
        BrowseNamespace::Hierarchical,
        branches.clone(),
        items.clone(),
        vec![],
    );
    let second_server = MockServer::da2(BrowseNamespace::Hierarchical, branches, items, vec![]);
    let mut sessions = BrowseSessions::default();
    let first_session = sessions.open(first_server).unwrap();
    let second_session = sessions.open(second_server).unwrap();

    let branches = sessions
        .page(
            &first_session,
            request(None, BrowseNodeFilter::Branches, 10, None),
        )
        .unwrap();
    sessions
        .page(
            &first_session,
            request(
                Some(branches.nodes[0].token),
                BrowseNodeFilter::Items,
                10,
                None,
            ),
        )
        .unwrap();

    let second_root = sessions
        .page(
            &second_session,
            request(None, BrowseNodeFilter::Items, 10, None),
        )
        .unwrap();
    assert_eq!(second_root.nodes[0].item_id.as_deref(), Some("exact::Root"));
}

#[test]
fn invalid_and_closed_sessions_are_rejected() {
    let mut sessions = BrowseSessions::<MockServer>::default();
    let invalid = BrowseSessionToken::new();
    assert!(
        sessions
            .page(&invalid, request(None, BrowseNodeFilter::All, 10, None))
            .is_err()
    );

    let session = sessions
        .open(MockServer::da2(
            BrowseNamespace::Flat,
            HashMap::new(),
            HashMap::new(),
            vec![],
        ))
        .unwrap();
    sessions.close(&session).unwrap();
    assert!(
        sessions
            .page(&session, request(None, BrowseNodeFilter::All, 10, None))
            .is_err()
    );
    assert!(sessions.close(&session).is_err());
}

#[test]
fn close_and_expiry_drop_session_owned_connections() {
    let close_drops = Arc::new(AtomicUsize::new(0));
    let expiry_drops = Arc::new(AtomicUsize::new(0));
    let mut sessions = BrowseSessions::default();
    let closed = sessions
        .open(
            MockServer::da2(
                BrowseNamespace::Flat,
                HashMap::new(),
                HashMap::new(),
                vec![],
            )
            .with_drop_count(close_drops.clone()),
        )
        .unwrap();
    sessions.close(&closed).unwrap();
    assert_eq!(close_drops.load(Ordering::Relaxed), 1);

    let expired = sessions
        .open(
            MockServer::da2(
                BrowseNamespace::Flat,
                HashMap::new(),
                HashMap::new(),
                vec![],
            )
            .with_drop_count(expiry_drops.clone()),
        )
        .unwrap();
    sessions.sessions.get_mut(&expired).unwrap().last_used = Instant::now()
        .checked_sub(Duration::from_secs(301))
        .unwrap();
    sessions.cleanup_expired();
    assert_eq!(expiry_drops.load(Ordering::Relaxed), 1);
    assert!(
        sessions
            .page(&expired, request(None, BrowseNodeFilter::All, 10, None))
            .is_err()
    );
}
