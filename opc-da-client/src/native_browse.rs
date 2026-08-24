use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, Da2BranchNavigation, NativeBrowseElement,
    NativeBrowsePage, classify_da2_branch,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT, OPC_NS_HIERARCHIAL,
};
use crate::opc_da::errors::{
    OpcError, OpcResult, com_hresult, contextual_browse_error, is_da3_browse_compatibility_error,
};
use crate::provider::{
    BrowseCapabilities, BrowseNamespace, BrowseNode, BrowseNodeFilter, BrowseNodeKind,
    BrowseNodeToken, BrowsePage, BrowsePageRequest, BrowsePageToken, BrowseSessionToken,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const MAX_BROWSE_PAGE_SIZE: u32 = 1_000;
const MAX_BROWSE_SESSIONS: usize = 64;
const MAX_NODE_TOKENS_PER_SESSION: usize = 100_000;
const MAX_PAGE_TOKENS_PER_SESSION: usize = 256;
const BROWSE_SESSION_IDLE_SECONDS: u64 = 300;
const BROWSE_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(BROWSE_SESSION_IDLE_SECONDS);

pub fn capabilities_for_server<S: ConnectedServer>(server: &S) -> OpcResult<BrowseCapabilities> {
    let supports_da3 = server.supports_da3_browse();
    let supports_da2 = server.supports_da2_browse();
    if !supports_da3 && !supports_da2 {
        return Err(OpcError::NotImplemented(
            "Server exposes neither OPC DA 3.0 nor OPC DA 2.x browsing".to_string(),
        ));
    }

    let namespace = if supports_da2 {
        match server.query_organization()? {
            value if value == OPC_NS_FLAT.0.cast_unsigned() => BrowseNamespace::Flat,
            value if value == OPC_NS_HIERARCHIAL.0.cast_unsigned() => BrowseNamespace::Hierarchical,
            value => {
                return Err(OpcError::Server(
                    "Server returned an unknown namespace organization".to_string(),
                    value,
                ));
            }
        }
    } else {
        BrowseNamespace::Unknown
    };

    Ok(BrowseCapabilities {
        namespace,
        supports_da3,
        supports_da2,
        max_page_size: MAX_BROWSE_PAGE_SIZE,
    })
}

pub struct BrowseSessions<S: ConnectedServer> {
    sessions: HashMap<BrowseSessionToken, BrowseSessionState<S>>,
}

impl<S: ConnectedServer> Default for BrowseSessions<S> {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

impl<S: ConnectedServer> BrowseSessions<S> {
    pub fn cleanup_expired(&mut self) {
        self.sessions
            .retain(|_, session| session.last_used.elapsed() < BROWSE_SESSION_IDLE_TIMEOUT);
    }

    pub fn open(&mut self, server: S) -> OpcResult<BrowseSessionToken> {
        self.cleanup_expired();
        if self.sessions.len() >= MAX_BROWSE_SESSIONS {
            return Err(OpcError::InvalidState(format!(
                "Maximum of {MAX_BROWSE_SESSIONS} browse sessions is already open"
            )));
        }

        let capabilities = capabilities_for_server(&server)?;
        let backend = if capabilities.supports_da3 {
            BrowseBackend::Da3
        } else {
            BrowseBackend::Da2 {
                current_path: Vec::new(),
            }
        };
        let token = unique_session_token(&self.sessions);
        self.sessions.insert(
            token,
            BrowseSessionState {
                server,
                capabilities,
                backend,
                da3_root_succeeded: false,
                nodes: HashMap::new(),
                continuations: HashMap::new(),
                last_used: Instant::now(),
            },
        );
        Ok(token)
    }

    pub fn page(
        &mut self,
        session_token: &BrowseSessionToken,
        request: BrowsePageRequest,
    ) -> OpcResult<BrowsePage> {
        self.cleanup_expired();
        if request.max_elements == 0 || request.max_elements > MAX_BROWSE_PAGE_SIZE {
            return Err(OpcError::InvalidState(format!(
                "Browse page size must be between 1 and {MAX_BROWSE_PAGE_SIZE}"
            )));
        }

        let session = self.sessions.get_mut(session_token).ok_or_else(|| {
            OpcError::InvalidState("Browse session is invalid, closed, or expired".to_string())
        })?;
        session.last_used = Instant::now();

        if session.capabilities.namespace == BrowseNamespace::Flat && request.parent.is_some() {
            return Err(OpcError::InvalidState(
                "Flat namespaces do not have browsable child nodes".to_string(),
            ));
        }

        match request.continuation {
            Some(token) => {
                let continuation = session.continuations.get(&token).ok_or_else(|| {
                    OpcError::InvalidState(
                        "Browse continuation is invalid, expired, or already consumed".to_string(),
                    )
                })?;
                if continuation.parent() != request.parent
                    || continuation.filter() != request.filter
                {
                    return Err(OpcError::InvalidState(
                        "Browse continuation does not match the requested parent and filter"
                            .to_string(),
                    ));
                }
                let Some(continuation) = session.continuations.remove(&token) else {
                    return Err(OpcError::Internal(
                        "Validated browse continuation disappeared".to_string(),
                    ));
                };
                Self::continue_page(session, continuation, request.max_elements)
            }
            None => Self::first_page(
                session,
                request.parent,
                request.filter,
                request.max_elements,
            ),
        }
    }

    pub fn close(&mut self, token: &BrowseSessionToken) -> OpcResult<()> {
        self.cleanup_expired();
        self.sessions.remove(token).map_or_else(
            || {
                Err(OpcError::InvalidState(
                    "Browse session is invalid, closed, or expired".to_string(),
                ))
            },
            |_| Ok(()),
        )
    }

    fn first_page(
        session: &mut BrowseSessionState<S>,
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        max_elements: u32,
    ) -> OpcResult<BrowsePage> {
        match session.backend {
            BrowseBackend::Da3 => {
                let can_fallback = parent.is_none() && !session.da3_root_succeeded;
                match Self::browse_da3(session, parent, filter, max_elements, None) {
                    Ok(page) => {
                        if parent.is_none() {
                            session.da3_root_succeeded = true;
                        }
                        Ok(page)
                    }
                    Err(error)
                        if can_fallback
                            && session.capabilities.supports_da2
                            && is_da3_browse_compatibility_error(&error) =>
                    {
                        tracing::warn!(
                            hresult = com_hresult(&error)
                                .map(|value| format!("0x{value:08X}"))
                                .as_deref()
                                .unwrap_or("N/A"),
                            error = %error,
                            "OPC DA 3.0 root browse is incompatible; falling back to OPC DA 2.x"
                        );
                        session.capabilities.supports_da3 = false;
                        session.backend = BrowseBackend::Da2 {
                            current_path: Vec::new(),
                        };
                        let state = Self::start_da2_page(session, parent, filter)?;
                        Self::browse_da2(session, parent, filter, max_elements, state)
                    }
                    result => result,
                }
            }
            BrowseBackend::Da2 { .. } => {
                let state = Self::start_da2_page(session, parent, filter)?;
                Self::browse_da2(session, parent, filter, max_elements, state)
            }
        }
    }

    fn continue_page(
        session: &mut BrowseSessionState<S>,
        continuation: BrowseContinuation,
        max_elements: u32,
    ) -> OpcResult<BrowsePage> {
        match continuation {
            BrowseContinuation::Da3 {
                parent,
                filter,
                raw,
            } => Self::browse_da3(session, parent, filter, max_elements, Some(&raw)),
            BrowseContinuation::Da2 {
                parent,
                filter,
                state,
            } => Self::browse_da2(session, parent, filter, max_elements, state),
        }
    }

    fn browse_da3(
        session: &mut BrowseSessionState<S>,
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        max_elements: u32,
        continuation: Option<&str>,
    ) -> OpcResult<BrowsePage> {
        let item_id = match parent {
            Some(token) => {
                let node = session.nodes.get(&token).ok_or_else(|| {
                    OpcError::InvalidState(
                        "Browse parent node is invalid or belongs to another session".to_string(),
                    )
                })?;
                if !node.kind.has_children() {
                    return Err(OpcError::InvalidState(
                        "Browse parent node has no children".to_string(),
                    ));
                }
                match &node.location {
                    NodeLocation::Da3(item_id) => Some(item_id.clone()),
                    NodeLocation::Da2(_) | NodeLocation::Item => {
                        return Err(OpcError::InvalidState(
                            "Browse parent node is incompatible with this session".to_string(),
                        ));
                    }
                }
            }
            None => None,
        };

        let NativeBrowsePage {
            elements,
            more_elements,
            continuation: raw_continuation,
        } = session
            .server
            .browse_da3(item_id.as_deref(), continuation, max_elements, filter)?;
        let nodes = Self::map_da3_nodes(session, elements)?;
        let continuation =
            Self::store_da3_continuation(session, parent, filter, more_elements, raw_continuation)?;

        Ok(BrowsePage {
            nodes,
            continuation,
        })
    }

    fn map_da3_nodes(
        session: &mut BrowseSessionState<S>,
        elements: Vec<NativeBrowseElement>,
    ) -> OpcResult<Vec<BrowseNode>> {
        let mut nodes = Vec::with_capacity(elements.len());
        for element in elements {
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
            if matches!(kind, BrowseNodeKind::Item | BrowseNodeKind::BranchAndItem)
                && element.item_id.is_none()
            {
                return Err(OpcError::Internal(format!(
                    "DA3 item '{}' did not include an item ID",
                    element.name
                )));
            }
            let location = if kind.has_children() {
                NodeLocation::Da3(element.item_id.clone().ok_or_else(|| {
                    OpcError::Internal(format!(
                        "DA3 branch '{}' did not include an item ID",
                        element.name
                    ))
                })?)
            } else {
                NodeLocation::Item
            };
            let token = insert_node(session, kind, location)?;
            nodes.push(BrowseNode {
                token,
                name: element.name,
                item_id: element.item_id,
                kind,
            });
        }
        Ok(nodes)
    }

    fn store_da3_continuation(
        session: &mut BrowseSessionState<S>,
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        more_elements: bool,
        raw_continuation: Option<String>,
    ) -> OpcResult<Option<BrowsePageToken>> {
        if !more_elements {
            return Ok(None);
        }
        let raw = raw_continuation.ok_or_else(|| {
            OpcError::Internal(
                "DA3 server reported more elements without a continuation point".to_string(),
            )
        })?;
        store_continuation(
            session,
            BrowseContinuation::Da3 {
                parent,
                filter,
                raw,
            },
        )
        .map(Some)
    }

    fn start_da2_page(
        session: &mut BrowseSessionState<S>,
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
    ) -> OpcResult<Da2PageState> {
        let parent_path = match parent {
            Some(token) => {
                let node = session.nodes.get(&token).ok_or_else(|| {
                    OpcError::InvalidState(
                        "Browse parent node is invalid or belongs to another session".to_string(),
                    )
                })?;
                if !node.kind.has_children() {
                    return Err(OpcError::InvalidState(
                        "Browse parent node has no children".to_string(),
                    ));
                }
                match &node.location {
                    NodeLocation::Da2(path) => path.clone(),
                    NodeLocation::Da3(_) | NodeLocation::Item => {
                        return Err(OpcError::InvalidState(
                            "Browse parent node is incompatible with this session".to_string(),
                        ));
                    }
                }
            }
            None => Vec::new(),
        };

        if session.capabilities.namespace == BrowseNamespace::Flat {
            let items = if filter == BrowseNodeFilter::Branches {
                None
            } else {
                Some(BufferedBrowseIterator::new(
                    session
                        .server
                        .begin_da2_browse(OPC_FLAT.0.cast_unsigned(), Some(""), 0, 0)?,
                ))
            };
            return Ok(Da2PageState {
                parent_path,
                branches: None,
                items,
                flat: true,
                merged_items: HashSet::new(),
            });
        }

        move_to_da2_path(session, &parent_path)?;
        let branches = if filter == BrowseNodeFilter::Items {
            None
        } else {
            Some(BufferedBrowseIterator::new(
                session
                    .server
                    .begin_da2_browse(OPC_BRANCH.0.cast_unsigned(), Some(""), 0, 0)?,
            ))
        };
        let items = if filter == BrowseNodeFilter::Branches {
            None
        } else {
            Some(BufferedBrowseIterator::new(
                session
                    .server
                    .begin_da2_browse(OPC_LEAF.0.cast_unsigned(), Some(""), 0, 0)?,
            ))
        };
        Ok(Da2PageState {
            parent_path,
            branches,
            items,
            flat: false,
            merged_items: HashSet::new(),
        })
    }

    fn browse_da2(
        session: &mut BrowseSessionState<S>,
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        max_elements: u32,
        mut state: Da2PageState,
    ) -> OpcResult<BrowsePage> {
        if !state.flat {
            move_to_da2_path(session, &state.parent_path)?;
        }

        let mut nodes = Vec::with_capacity(max_elements as usize);
        while nodes.len() < max_elements as usize {
            let Some((mut kind, name)) = state.next()? else {
                break;
            };
            if kind == BrowseNodeKind::Item && state.merged_items.contains(&name) {
                continue;
            }
            let (item_id, location) = match kind {
                BrowseNodeKind::Branch => {
                    let Some((mapped_kind, item_id, location)) =
                        map_browse_da2_branch(&session.server, &mut state, filter, &name)?
                    else {
                        continue;
                    };
                    kind = mapped_kind;
                    (item_id, location)
                }
                BrowseNodeKind::Item => {
                    let item_id = if state.flat {
                        name.clone()
                    } else {
                        session.server.get_item_id(&name).map_err(|error| {
                            contextual_browse_error(
                                error,
                                "get_item_id",
                                &state.parent_path,
                                Some(&name),
                            )
                        })?
                    };
                    if !state.flat
                        && session
                            .server
                            .da2_name_has_children(&name)
                            .map_err(|error| {
                                contextual_browse_error(
                                    error,
                                    "probe_da2_branch",
                                    &state.parent_path,
                                    Some(&name),
                                )
                            })?
                    {
                        let mut path = state.parent_path.clone();
                        path.push(name.clone());
                        kind = BrowseNodeKind::BranchAndItem;
                        (Some(item_id), NodeLocation::Da2(path))
                    } else {
                        (Some(item_id), NodeLocation::Item)
                    }
                }
                BrowseNodeKind::BranchAndItem => {
                    return Err(OpcError::Internal(
                        "DA2 browse returned an impossible combined node kind".to_string(),
                    ));
                }
            };
            let token = insert_node(session, kind, location)?;
            nodes.push(BrowseNode {
                token,
                name,
                item_id,
                kind,
            });
        }

        let continuation = if state.has_more() {
            Some(store_continuation(
                session,
                BrowseContinuation::Da2 {
                    parent,
                    filter,
                    state,
                },
            )?)
        } else {
            None
        };

        Ok(BrowsePage {
            nodes,
            continuation,
        })
    }
}

fn map_browse_da2_branch<S: ConnectedServer>(
    server: &S,
    state: &mut Da2PageState,
    filter: BrowseNodeFilter,
    name: &str,
) -> OpcResult<Option<(BrowseNodeKind, Option<String>, NodeLocation)>> {
    let mut path = state.parent_path.clone();
    path.push(name.to_string());
    let classification = classify_da2_branch(server, name).map_err(|error| {
        contextual_browse_error(error, "classify_da2_branch", &state.parent_path, Some(name))
    })?;
    Ok(match (classification.item_id, classification.navigation) {
        (Some(item_id), Da2BranchNavigation::Navigable) => {
            state.merged_items.insert(name.to_string());
            Some((
                BrowseNodeKind::BranchAndItem,
                Some(item_id),
                NodeLocation::Da2(path),
            ))
        }
        (Some(item_id), Da2BranchNavigation::RejectedInvalidArgument) => {
            state.merged_items.insert(name.to_string());
            if filter == BrowseNodeFilter::Branches {
                return Ok(None);
            }
            tracing::debug!(
                browse_path = ?state.parent_path,
                item_name = ?name,
                hresult = "0x80070057",
                "preserving exact DA2 item returned as a non-navigable branch"
            );
            Some((BrowseNodeKind::Item, Some(item_id), NodeLocation::Item))
        }
        (None, Da2BranchNavigation::Navigable) => {
            Some((BrowseNodeKind::Branch, None, NodeLocation::Da2(path)))
        }
        (None, Da2BranchNavigation::RejectedInvalidArgument) => {
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

struct BrowseSessionState<S: ConnectedServer> {
    server: S,
    capabilities: BrowseCapabilities,
    backend: BrowseBackend,
    da3_root_succeeded: bool,
    nodes: HashMap<BrowseNodeToken, NodeState>,
    continuations: HashMap<BrowsePageToken, BrowseContinuation>,
    last_used: Instant,
}

enum BrowseBackend {
    Da3,
    Da2 { current_path: Vec<String> },
}

struct NodeState {
    kind: BrowseNodeKind,
    location: NodeLocation,
}

enum NodeLocation {
    Da3(String),
    Da2(Vec<String>),
    Item,
}

enum BrowseContinuation {
    Da3 {
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        raw: String,
    },
    Da2 {
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        state: Da2PageState,
    },
}

impl BrowseContinuation {
    fn parent(&self) -> Option<BrowseNodeToken> {
        match self {
            Self::Da3 { parent, .. } | Self::Da2 { parent, .. } => *parent,
        }
    }

    fn filter(&self) -> BrowseNodeFilter {
        match self {
            Self::Da3 { filter, .. } | Self::Da2 { filter, .. } => *filter,
        }
    }
}

struct Da2PageState {
    parent_path: Vec<String>,
    branches: Option<BufferedBrowseIterator>,
    items: Option<BufferedBrowseIterator>,
    flat: bool,
    merged_items: HashSet<String>,
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
    session: &mut BrowseSessionState<S>,
    target: &[String],
) -> OpcResult<()> {
    let BrowseBackend::Da2 { current_path } = &mut session.backend else {
        return Err(OpcError::InvalidState(
            "DA2 browse position requested for a DA3 session".to_string(),
        ));
    };
    let shared = current_path
        .iter()
        .zip(target)
        .take_while(|(left, right)| left == right)
        .count();

    for _ in shared..current_path.len() {
        session
            .server
            .change_browse_position(OPC_BROWSE_UP.0.cast_unsigned(), "")?;
    }
    current_path.truncate(shared);

    for branch in &target[shared..] {
        session
            .server
            .change_browse_position(OPC_BROWSE_DOWN.0.cast_unsigned(), branch)?;
        current_path.push(branch.clone());
    }
    Ok(())
}

fn insert_node<S: ConnectedServer>(
    session: &mut BrowseSessionState<S>,
    kind: BrowseNodeKind,
    location: NodeLocation,
) -> OpcResult<BrowseNodeToken> {
    if session.nodes.len() >= MAX_NODE_TOKENS_PER_SESSION {
        return Err(OpcError::InvalidState(format!(
            "Browse session reached its limit of {MAX_NODE_TOKENS_PER_SESSION} node tokens"
        )));
    }
    let token = loop {
        let candidate = BrowseNodeToken::new();
        if !session.nodes.contains_key(&candidate) {
            break candidate;
        }
    };
    session.nodes.insert(token, NodeState { kind, location });
    Ok(token)
}

fn store_continuation<S: ConnectedServer>(
    session: &mut BrowseSessionState<S>,
    continuation: BrowseContinuation,
) -> OpcResult<BrowsePageToken> {
    if session.continuations.len() >= MAX_PAGE_TOKENS_PER_SESSION {
        return Err(OpcError::InvalidState(format!(
            "Browse session reached its limit of {MAX_PAGE_TOKENS_PER_SESSION} page tokens"
        )));
    }
    let token = loop {
        let candidate = BrowsePageToken::new();
        if !session.continuations.contains_key(&candidate) {
            break candidate;
        }
    };
    session.continuations.insert(token, continuation);
    Ok(token)
}

fn unique_session_token<S: ConnectedServer>(
    sessions: &HashMap<BrowseSessionToken, BrowseSessionState<S>>,
) -> BrowseSessionToken {
    loop {
        let candidate = BrowseSessionToken::new();
        if !sessions.contains_key(&candidate) {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::connector::{ConnectedGroup, RemoteArray};
    use crate::bindings::da::{tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE};
    use crate::opc_da::errors::{E_INVALIDARG_HRESULT, RPC_X_NULL_REF_POINTER_HRESULT};
    use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use windows::Win32::System::Variant::VARIANT;
    use windows::core::HRESULT;

    type Da3Call = (Option<String>, Option<String>, BrowseNodeFilter);

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
                if let Some(hresult) = self.navigation_errors.get(name) {
                    return Err(OpcError::Com {
                        source: windows::core::Error::from_hresult(HRESULT(
                            (*hresult).cast_signed(),
                        )),
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
    fn da3_root_compatibility_failure_falls_back_to_da2_for_the_session() {
        let server = MockServer::da3(Vec::new()).with_da2_fallback(
            RPC_X_NULL_REF_POINTER_HRESULT,
            vec!["Channel.Device.Tag".to_string()],
        );
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
        assert!(second.nodes.is_empty());
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
        assert!(branches_only.nodes.is_empty());
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
}
