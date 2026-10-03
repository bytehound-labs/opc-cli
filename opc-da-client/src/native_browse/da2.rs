//! DA2 immediate children, buffering, and session cursor navigation.

use crate::backend::connector::{
    BrowseStringIterator, ConnectedServer, Da2BranchNavigation, classify_da2_branch,
    guard_browse_iterator,
};
use crate::bindings::da::{OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_FLAT, OPC_LEAF};
use crate::errors::{OpcError, OpcResult, contextual_browse_error, is_non_progress_browse_error};
use crate::native_browse::BrowseSessions;
use crate::native_browse::state::{
    BrowseBackend, BrowseContinuation, BrowseSessionState, NodeLocation, insert_node,
    store_continuation,
};
use crate::provider::{
    BrowseNamespace, BrowseNode, BrowseNodeFilter, BrowseNodeKind, BrowseNodeToken, BrowsePage,
};
use std::collections::HashSet;

impl<S: ConnectedServer> BrowseSessions<S> {
    pub(super) fn start_da2_page(
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
                    "native DA2 flat iterator",
                    &parent_path,
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
                "native DA2 branch iterator",
                &parent_path,
            ))
        };
        let items = if filter == BrowseNodeFilter::Branches {
            None
        } else {
            Some(BufferedBrowseIterator::new(
                session
                    .server
                    .begin_da2_browse(OPC_LEAF.0.cast_unsigned(), Some(""), 0, 0)?,
                "native DA2 item iterator",
                &parent_path,
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

    pub(super) fn browse_da2(
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
                    state: Box::new(state),
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
            tracing::debug!(target: "opc_da_client::native_browse",
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
            tracing::warn!(target: "opc_da_client::native_browse",
                browse_path = ?state.parent_path,
                item_name = ?name,
                hresult = "0x80070057",
                "skipping non-navigable DA2 branch-only name"
            );
            None
        }
    })
}

pub(super) struct Da2PageState {
    pub(super) parent_path: Vec<String>,
    pub(super) branches: Option<BufferedBrowseIterator>,
    pub(super) items: Option<BufferedBrowseIterator>,
    pub(super) flat: bool,
    pub(super) merged_items: HashSet<String>,
}

impl Da2PageState {
    pub(super) fn next(&mut self) -> OpcResult<Option<(BrowseNodeKind, String)>> {
        let branch_result = self.branches.as_mut().map(BufferedBrowseIterator::next);
        match branch_result {
            Some(Some(Ok(name))) => return Ok(Some((BrowseNodeKind::Branch, name))),
            Some(Some(Err(error))) if is_non_progress_browse_error(&error) => {
                self.skip_non_progressing_branch(&error);
            }
            Some(Some(Err(error))) => return Err(error),
            Some(None) => self.branches = None,
            None => {}
        }

        let item_result = self.items.as_mut().map(BufferedBrowseIterator::next);
        match item_result {
            Some(Some(Ok(name))) => return Ok(Some((BrowseNodeKind::Item, name))),
            Some(Some(Err(error))) => return Err(error),
            Some(None) => self.items = None,
            None => {}
        }

        Ok(None)
    }

    pub(super) fn has_more(&mut self) -> bool {
        let branch_has_more = self
            .branches
            .as_mut()
            .is_some_and(BufferedBrowseIterator::has_more);
        if branch_has_more {
            if let Some(error) = self
                .branches
                .as_mut()
                .and_then(BufferedBrowseIterator::take_non_progress)
            {
                self.skip_non_progressing_branch(&error);
            } else {
                return true;
            }
        }

        self.items
            .as_mut()
            .is_some_and(BufferedBrowseIterator::has_more)
    }

    fn skip_non_progressing_branch(&mut self, error: &OpcError) {
        tracing::warn!(target: "opc_da_client::native_browse",
            browse_path = ?self.parent_path,
            error = ?error,
            "skipping non-progressing DA2 branch iterator and continuing with item iterator"
        );
        self.branches = None;
    }
}

pub(super) struct BufferedBrowseIterator {
    pub(super) inner: Box<dyn BrowseStringIterator>,
    pub(super) pending: Option<OpcResult<String>>,
}

impl BufferedBrowseIterator {
    pub(super) fn new(
        inner: Box<dyn BrowseStringIterator>,
        iterator_type: &str,
        browse_path: &[String],
    ) -> Self {
        Self {
            inner: guard_browse_iterator(inner, iterator_type, browse_path),
            pending: None,
        }
    }

    pub(super) fn next(&mut self) -> Option<OpcResult<String>> {
        self.pending.take().or_else(|| self.inner.next_string())
    }

    fn has_more(&mut self) -> bool {
        if self.pending.is_none() {
            self.pending = self.inner.next_string();
        }
        self.pending.is_some()
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
