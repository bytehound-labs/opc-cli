//! Canonical DA2 browse-to with classified component fallback.

use crate::backend::connector::ConnectedServer;
use crate::bindings::da::{OPC_BROWSE_DOWN, OPC_BROWSE_UP};
use crate::errors::{
    E_INVALIDARG_HRESULT, contextual_browse_error, is_com_hresult, is_da2_browse_to_fallback_error,
};
use crate::inventory::state::Da2Path;
use crate::inventory_boundary::{InventoryBoundary, paced_call};
use crate::inventory_error::{InventoryError, contextual_inventory_error};
use crate::provider::InventoryNativeOperationKind;

pub(super) fn move_to_da2_path<S: ConnectedServer>(
    server: &S,
    current_path: &mut Vec<String>,
    target: &Da2Path,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<(), InventoryError> {
    if let Some(item_id) = target.item_id.as_deref()
        && *current_path != target.components
    {
        match paced_call(boundary, InventoryNativeOperationKind::Da2PathTo, 1, || {
            server.change_browse_position_to(item_id)
        }) {
            Ok(()) => {
                current_path.clone_from(&target.components);
                return Ok(());
            }
            Err(InventoryError::Failed(error)) if is_da2_browse_to_fallback_error(&error) => {}
            Err(InventoryError::Cancelled) => return Err(InventoryError::Cancelled),
            Err(InventoryError::Failed(error)) => {
                return Err(contextual_browse_error(
                    error,
                    "change_browse_position_to",
                    &target.components,
                    Some(item_id),
                )
                .into());
            }
            Err(error) => return Err(error),
        }
    }

    let target_components = &target.components;
    let shared = current_path
        .iter()
        .zip(target_components)
        .take_while(|(left, right)| left == right)
        .count();
    for _ in shared..current_path.len() {
        paced_call(boundary, InventoryNativeOperationKind::Da2PathUp, 1, || {
            server.change_browse_position(OPC_BROWSE_UP.0.cast_unsigned(), "")
        })
        .map_err(|error| {
            contextual_inventory_error(
                error,
                "change_browse_position(up)",
                current_path,
                current_path.last().map(String::as_str),
            )
        })?;
    }
    current_path.truncate(shared);
    for branch in &target_components[shared..] {
        match paced_call(
            boundary,
            InventoryNativeOperationKind::Da2PathDown,
            1,
            || server.change_browse_position(OPC_BROWSE_DOWN.0.cast_unsigned(), branch),
        ) {
            Ok(()) => current_path.push(branch.clone()),
            Err(InventoryError::Failed(error)) if is_com_hresult(&error, E_INVALIDARG_HRESULT) => {
                return Err(InventoryError::InvalidDa2Branch {
                    parent_path: current_path.clone(),
                    branch: branch.clone(),
                });
            }
            Err(error) => {
                return Err(contextual_inventory_error(
                    error,
                    "change_browse_position(down)",
                    current_path,
                    Some(branch),
                ));
            }
        }
    }
    Ok(())
}
