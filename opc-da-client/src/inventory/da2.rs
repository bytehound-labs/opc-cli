//! Deferred DA2 branch expansion and bounded page state.

use crate::backend::connector::ConnectedServer;
use crate::bindings::da::{OPC_BRANCH, OPC_FLAT, OPC_LEAF};
use crate::errors::{
    E_INVALIDARG_HRESULT, OpcError, contextual_browse_error, is_com_hresult,
    is_non_progress_browse_error,
};
use crate::inventory::iterator::BufferedBrowseIterator;
use crate::inventory::navigation::move_to_da2_path;
use crate::inventory::progress::describe_browse_path;
use crate::inventory::state::{BranchLocation, Da2Path, InventoryDa2BranchNode, InventoryNode};
use crate::inventory_boundary::{InventoryBoundary, paced_call};
use crate::inventory_error::{InventoryError, contextual_inventory_error};
use crate::provider::{BrowseNamespace, BrowseNodeKind, InventoryNativeOperationKind};
use std::collections::HashSet;

pub(super) struct Da2PageState {
    pub(super) parent_path: Vec<String>,
    pub(super) parent_item_id: Option<String>,
    pub(super) branches: Option<BufferedBrowseIterator>,
    pub(super) items: Option<BufferedBrowseIterator>,
    pub(super) flat: bool,
    pub(super) merged_items: HashSet<String>,
}

pub(super) struct InventoryPageContext<'a, 'control> {
    pub(super) current_da2_path: &'a mut Vec<String>,
    pub(super) namespace: BrowseNamespace,
    pub(super) skipped_invalid_branches: &'a mut u64,
    pub(super) first_skipped_invalid_branch: &'a mut Option<String>,
    pub(super) skipped_non_progressing_branches: &'a mut u64,
    pub(super) first_skipped_non_progressing_branch: &'a mut Option<String>,
    pub(super) boundary: &'a mut InventoryBoundary<'control>,
}

pub(super) fn start_da2_page<S: ConnectedServer>(
    server: &S,
    parent_path: &Da2Path,
    current_path: &mut Vec<String>,
    namespace: BrowseNamespace,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<Da2PageState, InventoryError> {
    move_to_da2_path(server, current_path, parent_path, boundary)?;
    let parent_components = &parent_path.components;
    let flat = matches!(namespace, BrowseNamespace::Flat);
    let branches = if flat {
        None
    } else {
        let iterator = paced_call(
            boundary,
            InventoryNativeOperationKind::Da2BranchEnumeratorCreation,
            1,
            || server.begin_da2_browse(OPC_BRANCH.0.cast_unsigned(), Some(""), 0, 0),
        )
        .map_err(|error| match error {
            InventoryError::Failed(error) => InventoryError::Failed(contextual_browse_error(
                error,
                "begin_da2_browse(branches)",
                parent_components,
                None,
            )),
            InventoryError::Cancelled => InventoryError::Cancelled,
            error @ InventoryError::InvalidDa2Branch { .. } => error,
        })?;
        Some(BufferedBrowseIterator::new(
            iterator,
            "inventory DA2 branch iterator",
            parent_components,
        ))
    };
    let iterator = paced_call(
        boundary,
        if flat {
            InventoryNativeOperationKind::Da2FlatEnumeratorCreation
        } else {
            InventoryNativeOperationKind::Da2LeafEnumeratorCreation
        },
        1,
        || {
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
        },
    )
    .map_err(|error| match error {
        InventoryError::Failed(error) => InventoryError::Failed(contextual_browse_error(
            error,
            if flat {
                "begin_da2_browse(flat)"
            } else {
                "begin_da2_browse(items)"
            },
            parent_components,
            None,
        )),
        InventoryError::Cancelled => InventoryError::Cancelled,
        error @ InventoryError::InvalidDa2Branch { .. } => error,
    })?;
    let items = Some(BufferedBrowseIterator::new(
        iterator,
        if flat {
            "inventory DA2 flat iterator"
        } else {
            "inventory DA2 item iterator"
        },
        parent_components,
    ));
    Ok(Da2PageState {
        parent_path: parent_components.clone(),
        parent_item_id: parent_path.item_id.clone(),
        branches,
        items,
        flat,
        merged_items: HashSet::new(),
    })
}

pub(super) fn browse_da2_page<S: ConnectedServer>(
    server: &S,
    mut state: Da2PageState,
    batch_size: u32,
    context: &mut InventoryPageContext<'_, '_>,
) -> Result<(Vec<InventoryNode>, Option<Da2PageState>), InventoryError> {
    move_to_da2_path(
        server,
        context.current_da2_path,
        &Da2Path {
            components: state.parent_path.clone(),
            item_id: state.parent_item_id.clone(),
        },
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
                let Some(mapped) =
                    map_inventory_da2_branch(server, &mut state, &name, context.boundary)?
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
                    match paced_call(
                        context.boundary,
                        InventoryNativeOperationKind::GetItemId,
                        1,
                        || server.get_item_id(&name),
                    ) {
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
                        Err(error @ InventoryError::InvalidDa2Branch { .. }) => {
                            return Err(error);
                        }
                    }
                };
                (Some(item_id), None)
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

pub(super) fn map_inventory_da2_branch<S: ConnectedServer>(
    server: &S,
    state: &mut Da2PageState,
    name: &str,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<Option<InventoryDa2BranchNode>, InventoryError> {
    let mut child_path = state.parent_path.clone();
    child_path.push(name.to_string());
    let item_id = match paced_call(boundary, InventoryNativeOperationKind::GetItemId, 1, || {
        server.resolve_da2_item_id(name)
    }) {
        Ok(item_id) => item_id,
        Err(InventoryError::Failed(error)) if is_com_hresult(&error, E_INVALIDARG_HRESULT) => None,
        Err(InventoryError::Failed(error)) => {
            return Err(contextual_browse_error(
                error,
                "resolve_da2_item_id(get_item_id)",
                &state.parent_path,
                Some(name),
            )
            .into());
        }
        Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
        Err(error @ InventoryError::InvalidDa2Branch { .. }) => return Err(error),
    };
    let kind = if item_id.is_some() {
        state.merged_items.insert(name.to_string());
        BrowseNodeKind::BranchAndItem
    } else {
        BrowseNodeKind::Branch
    };
    Ok(Some(InventoryDa2BranchNode {
        kind,
        item_id: item_id.clone(),
        child: Some(BranchLocation::Da2(Da2Path {
            components: child_path,
            item_id,
        })),
    }))
}

impl Da2PageState {
    pub(super) fn next(
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

    pub(super) fn has_more(
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

    pub(super) fn skip_non_progressing_branch(
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
        tracing::warn!(target: "opc_da_client::inventory",
            browse_path = ?self.parent_path,
            error = ?error,
            "skipping non-progressing DA2 branch iterator and continuing with item iterator"
        );
        self.branches = None;
    }
}
