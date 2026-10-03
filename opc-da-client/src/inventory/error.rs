//! Portable inventory cancellation and contextual errors.

use crate::errors::{OpcError, contextual_browse_error};

#[derive(Debug)]
pub enum InventoryError {
    Cancelled,
    Failed(OpcError),
    InvalidDa2Branch {
        parent_path: Vec<String>,
        branch: String,
    },
}

impl From<OpcError> for InventoryError {
    fn from(error: OpcError) -> Self {
        Self::Failed(error)
    }
}

pub fn contextual_inventory_error(
    error: InventoryError,
    operation: &str,
    path: &[String],
    item: Option<&str>,
) -> InventoryError {
    match error {
        InventoryError::Failed(error) => {
            InventoryError::Failed(contextual_browse_error(error, operation, path, item))
        }
        InventoryError::Cancelled => InventoryError::Cancelled,
        error @ InventoryError::InvalidDa2Branch { .. } => error,
    }
}

#[cfg(test)]
#[path = "../tests/inventory_error.rs"]
mod tests;
