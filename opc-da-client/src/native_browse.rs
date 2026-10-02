//! Worker-owned browse session lifecycle and one-level page dispatch.

mod capabilities;
mod da2;
mod da3;
mod state;

pub use capabilities::{MAX_BROWSE_PAGE_SIZE, capabilities_for_server};

use crate::backend::connector::ConnectedServer;
use crate::errors::{OpcError, OpcResult, com_hresult, is_da3_browse_compatibility_error};
use crate::native_browse::state::{
    BROWSE_SESSION_IDLE_TIMEOUT, BrowseBackend, BrowseContinuation, BrowseSessionState,
    MAX_BROWSE_SESSIONS, unique_session_token,
};
use crate::provider::{
    BrowseNamespace, BrowseNodeFilter, BrowseNodeToken, BrowsePage, BrowsePageRequest,
    BrowseSessionToken,
};
use std::collections::HashMap;
use std::time::Instant;

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
            } => Self::browse_da2(session, parent, filter, max_elements, *state),
        }
    }
}

#[cfg(test)]
#[path = "tests/native_browse.rs"]
mod tests;
