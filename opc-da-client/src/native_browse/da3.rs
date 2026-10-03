//! DA3 page mapping and opaque continuation storage.

use crate::backend::connector::{ConnectedServer, NativeBrowseElement, NativeBrowsePage};
use crate::errors::{OpcError, OpcResult};
use crate::native_browse::BrowseSessions;
use crate::native_browse::state::{
    BrowseContinuation, BrowseSessionState, NodeLocation, insert_node, store_continuation,
};
use crate::provider::{
    BrowseNode, BrowseNodeFilter, BrowseNodeKind, BrowseNodeToken, BrowsePage, BrowsePageToken,
};

impl<S: ConnectedServer> BrowseSessions<S> {
    pub(super) fn browse_da3(
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

    pub(super) fn map_da3_nodes(
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
            if kind.is_item() && element.item_id.is_none() {
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
            let item_id = if kind.is_item() {
                element.item_id
            } else {
                None
            };
            nodes.push(BrowseNode {
                token,
                name: element.name,
                item_id,
                kind,
            });
        }
        Ok(nodes)
    }

    pub(super) fn store_da3_continuation(
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
}
