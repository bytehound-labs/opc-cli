//! Buffered native enumeration with cached-item cancellation.

use crate::backend::connector::{BrowseStringIterator, guard_browse_iterator};
use crate::errors::{OpcError, OpcResult, is_non_progress_browse_error};
use crate::inventory_boundary::{BoundaryResult, InventoryBoundary};
use crate::inventory_error::InventoryError;

pub(super) struct BufferedBrowseIterator {
    inner: Box<dyn BrowseStringIterator>,
    pending: Option<OpcResult<String>>,
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

    pub(super) fn next(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
    ) -> Option<Result<String, InventoryError>> {
        if let Some(value) = self.pending.take() {
            return Some(value.map_err(InventoryError::from));
        }
        match self.next_native(boundary) {
            Ok(value) => value.map(|value| value.map_err(InventoryError::from)),
            Err(error) => Some(Err(error)),
        }
    }

    pub(super) fn has_more(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
    ) -> Result<bool, InventoryError> {
        if self.pending.is_none() {
            self.pending = self.next_native(boundary)?;
        }
        Ok(self.pending.is_some())
    }

    pub(super) fn next_native(
        &mut self,
        boundary: &mut InventoryBoundary<'_>,
    ) -> Result<Option<OpcResult<String>>, InventoryError> {
        let control = boundary.control();
        let mut cancelled = false;
        let mut before_native_operation =
            |item_cost| match boundary.before_operation_with_cost(item_cost) {
                BoundaryResult::Proceed => true,
                BoundaryResult::Cancelled => {
                    cancelled = true;
                    false
                }
            };
        let mut should_cancel = || control.is_cancelled();
        let value = self
            .inner
            .next_string_with_gate(&mut before_native_operation, &mut should_cancel);
        if cancelled || control.is_cancelled() {
            Err(InventoryError::Cancelled)
        } else {
            Ok(value)
        }
    }

    pub(super) fn take_non_progress(&mut self) -> Option<OpcError> {
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
