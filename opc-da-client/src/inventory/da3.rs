//! DA3 page mapping and branch-local continuation progress.

use crate::backend::connector::{ConnectedServer, NativeBrowseElement};
use crate::errors::{
    MAX_CONSECUTIVE_EMPTY_DA3_PAGES, OpcError, OpcResult, browse_continuation_non_progress_error,
    contextual_browse_error, is_da3_browse_compatibility_error,
};
use crate::inventory::da2::{InventoryPageContext, browse_da2_page, start_da2_page};
use crate::inventory::progress::record_skipped_invalid_branch;
use crate::inventory::state::{
    BranchLocation, BranchWork, InventoryContinuation, InventoryNode, InventoryPage,
};
use crate::inventory_boundary::paced_call;
use crate::inventory_error::InventoryError;
use crate::provider::{BrowseNodeFilter, BrowseNodeKind, InventoryNativeOperationKind};

pub(super) fn da3_continuation_error(work: &BranchWork, detail: String) -> InventoryError {
    InventoryError::Failed(browse_continuation_non_progress_error(
        &work.breadcrumbs,
        detail,
    ))
}

#[allow(clippy::too_many_lines)]
pub(super) fn next_page<S: ConnectedServer>(
    server: &S,
    work: &mut BranchWork,
    batch_size: u32,
    context: &mut InventoryPageContext<'_, '_>,
) -> Result<InventoryPage, InventoryError> {
    match &work.location {
        BranchLocation::Da3(item_id) => {
            let is_root = item_id.is_none() && work.breadcrumbs.is_empty();
            let page = paced_call(
                context.boundary,
                InventoryNativeOperationKind::Da3Page,
                batch_size,
                || {
                    server.browse_da3(
                        item_id.as_deref(),
                        work.da3_continuation.as_deref(),
                        batch_size,
                        BrowseNodeFilter::All,
                    )
                },
            )
            .map_err(|error| match error {
                InventoryError::Cancelled => InventoryError::Cancelled,
                InventoryError::Failed(error)
                    if is_root && is_da3_browse_compatibility_error(&error) =>
                {
                    InventoryError::Failed(error)
                }
                InventoryError::Failed(error) => InventoryError::Failed(contextual_browse_error(
                    error,
                    "browse_da3",
                    &work.breadcrumbs,
                    item_id.as_deref(),
                )),
                error @ InventoryError::InvalidDa2Branch { .. } => error,
            })?;
            let crate::backend::connector::NativeBrowsePage {
                elements,
                more_elements,
                continuation,
            } = page;
            let continuation = if more_elements {
                let value = continuation.as_deref().ok_or_else(|| {
                    da3_continuation_error(
                        work,
                        "server reported more elements without a continuation point".to_string(),
                    )
                })?;
                if value.is_empty() {
                    return Err(da3_continuation_error(
                        work,
                        "server returned an empty continuation token".to_string(),
                    ));
                }
                if !work.da3_seen_continuations.insert(value.to_string()) {
                    return Err(da3_continuation_error(
                        work,
                        format!("server repeated continuation token {value:?}"),
                    ));
                }
                if elements.is_empty() {
                    work.da3_consecutive_empty_pages =
                        work.da3_consecutive_empty_pages.saturating_add(1);
                    if work.da3_consecutive_empty_pages >= MAX_CONSECUTIVE_EMPTY_DA3_PAGES {
                        return Err(da3_continuation_error(
                            work,
                            format!(
                                "server returned {} consecutive empty pages",
                                work.da3_consecutive_empty_pages
                            ),
                        ));
                    }
                } else {
                    work.da3_consecutive_empty_pages = 0;
                }
                Some(value.to_string())
            } else {
                work.da3_consecutive_empty_pages = 0;
                None
            };
            let nodes = elements
                .into_iter()
                .map(map_da3_node)
                .collect::<OpcResult<Vec<_>>>()?;
            Ok(InventoryPage {
                nodes,
                continuation: continuation.map(InventoryContinuation::Da3),
            })
        }
        BranchLocation::Da2(path) => {
            if work.da2_state.is_none() {
                work.da2_state = Some(
                    match start_da2_page(
                        server,
                        path,
                        context.current_da2_path,
                        context.namespace,
                        context.boundary,
                    ) {
                        Ok(state) => state,
                        Err(InventoryError::InvalidDa2Branch {
                            parent_path,
                            branch,
                        }) => {
                            record_skipped_invalid_branch(
                                context.skipped_invalid_branches,
                                context.first_skipped_invalid_branch,
                                &parent_path,
                                &branch,
                            );
                            return Ok(InventoryPage {
                                nodes: Vec::new(),
                                continuation: None,
                            });
                        }
                        Err(error) => return Err(error),
                    },
                );
            }
            let state = work.da2_state.take().ok_or_else(|| {
                OpcError::Internal("DA2 inventory page state disappeared".to_string())
            })?;
            let (nodes, state) = match browse_da2_page(server, state, batch_size, context) {
                Ok(page) => page,
                Err(InventoryError::InvalidDa2Branch {
                    parent_path,
                    branch,
                }) => {
                    record_skipped_invalid_branch(
                        context.skipped_invalid_branches,
                        context.first_skipped_invalid_branch,
                        &parent_path,
                        &branch,
                    );
                    return Ok(InventoryPage {
                        nodes: Vec::new(),
                        continuation: None,
                    });
                }
                Err(error) => return Err(error),
            };
            Ok(InventoryPage {
                nodes,
                continuation: state.map(|state| InventoryContinuation::Da2(Box::new(state))),
            })
        }
    }
}

pub(super) fn map_da3_node(element: NativeBrowseElement) -> OpcResult<InventoryNode> {
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
