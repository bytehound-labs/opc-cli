//! Private traversal work, exact locations, and page state.

use crate::inventory::da2::Da2PageState;
use crate::provider::{BrowseCapabilities, BrowseNodeKind};
use std::collections::HashSet;

pub(super) struct BranchWork {
    pub(super) location: BranchLocation,
    pub(super) breadcrumbs: Vec<String>,
    pub(super) da3_continuation: Option<String>,
    pub(super) da3_seen_continuations: HashSet<String>,
    pub(super) da3_consecutive_empty_pages: usize,
    pub(super) da2_state: Option<Da2PageState>,
}

pub(super) enum BranchLocation {
    Da3(Option<String>),
    Da2(Da2Path),
}

pub(super) struct Da2Path {
    pub(super) components: Vec<String>,
    pub(super) item_id: Option<String>,
}

pub(super) struct InventoryNode {
    pub(super) display_name: String,
    pub(super) item_id: Option<String>,
    pub(super) kind: BrowseNodeKind,
    pub(super) child: Option<BranchLocation>,
}

pub(super) struct InventoryPage {
    pub(super) nodes: Vec<InventoryNode>,
    pub(super) continuation: Option<InventoryContinuation>,
}

pub(super) enum InventoryContinuation {
    Da3(String),
    Da2(Box<Da2PageState>),
}

pub(super) struct InventoryDa2BranchNode {
    pub(super) kind: BrowseNodeKind,
    pub(super) item_id: Option<String>,
    pub(super) child: Option<BranchLocation>,
}

pub(super) fn initial_work(
    capabilities: BrowseCapabilities,
    root_item_id: Option<&str>,
) -> BranchWork {
    let location = if capabilities.supports_da3 {
        BranchLocation::Da3(root_item_id.map(str::to_owned))
    } else {
        BranchLocation::Da2(Da2Path {
            components: root_item_id
                .map(|item_id| vec![item_id.to_owned()])
                .unwrap_or_default(),
            item_id: root_item_id.map(str::to_owned),
        })
    };
    BranchWork {
        location,
        breadcrumbs: root_item_id
            .map(|item_id| vec![item_id.to_owned()])
            .unwrap_or_default(),
        da3_continuation: None,
        da3_seen_continuations: HashSet::new(),
        da3_consecutive_empty_pages: 0,
        da2_state: None,
    }
}

pub(super) fn is_initial_da3_root(work: &BranchWork) -> bool {
    matches!(work.location, BranchLocation::Da3(None))
        && work.da3_continuation.is_none()
        && work.breadcrumbs.is_empty()
}
