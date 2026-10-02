//! Session ownership, token bounds, and private node locations.

use crate::backend::connector::ConnectedServer;
use crate::errors::{OpcError, OpcResult};
use crate::native_browse::da2::Da2PageState;
use crate::provider::{
    BrowseCapabilities, BrowseNodeFilter, BrowseNodeKind, BrowseNodeToken, BrowsePageToken,
    BrowseSessionToken,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(super) const MAX_BROWSE_SESSIONS: usize = 64;
pub(super) const MAX_NODE_TOKENS_PER_SESSION: usize = 100_000;
pub(super) const MAX_PAGE_TOKENS_PER_SESSION: usize = 256;
pub(super) const BROWSE_SESSION_IDLE_SECONDS: u64 = 300;
pub(super) const BROWSE_SESSION_IDLE_TIMEOUT: Duration =
    Duration::from_secs(BROWSE_SESSION_IDLE_SECONDS);

pub(super) struct BrowseSessionState<S: ConnectedServer> {
    pub(super) server: S,
    pub(super) capabilities: BrowseCapabilities,
    pub(super) backend: BrowseBackend,
    pub(super) da3_root_succeeded: bool,
    pub(super) nodes: HashMap<BrowseNodeToken, NodeState>,
    pub(super) continuations: HashMap<BrowsePageToken, BrowseContinuation>,
    pub(super) last_used: Instant,
}

pub(super) enum BrowseBackend {
    Da3,
    Da2 { current_path: Vec<String> },
}

pub(super) struct NodeState {
    pub(super) kind: BrowseNodeKind,
    pub(super) location: NodeLocation,
}

pub(super) enum NodeLocation {
    Da3(String),
    Da2(Vec<String>),
    Item,
}

pub(super) enum BrowseContinuation {
    Da3 {
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        raw: String,
    },
    Da2 {
        parent: Option<BrowseNodeToken>,
        filter: BrowseNodeFilter,
        state: Box<Da2PageState>,
    },
}

impl BrowseContinuation {
    pub(super) fn parent(&self) -> Option<BrowseNodeToken> {
        match self {
            Self::Da3 { parent, .. } | Self::Da2 { parent, .. } => *parent,
        }
    }

    pub(super) fn filter(&self) -> BrowseNodeFilter {
        match self {
            Self::Da3 { filter, .. } | Self::Da2 { filter, .. } => *filter,
        }
    }
}

pub(super) fn insert_node<S: ConnectedServer>(
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

pub(super) fn store_continuation<S: ConnectedServer>(
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

pub(super) fn unique_session_token<S: ConnectedServer>(
    sessions: &HashMap<BrowseSessionToken, BrowseSessionState<S>>,
) -> BrowseSessionToken {
    loop {
        let candidate = BrowseSessionToken::new();
        if !sessions.contains_key(&candidate) {
            return candidate;
        }
    }
}
