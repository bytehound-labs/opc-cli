//! Capability detection before inventory traversal.

use crate::backend::connector::ConnectedServer;
use crate::bindings::da::OPC_NS_FLAT;
use crate::errors::OpcError;
use crate::inventory_boundary::{InventoryBoundary, paced_call};
use crate::inventory_error::InventoryError;
use crate::provider::{BrowseCapabilities, InventoryNativeOperationKind};

pub(super) fn capabilities_for_inventory<S: ConnectedServer>(
    server: &S,
    boundary: &mut InventoryBoundary<'_>,
) -> Result<BrowseCapabilities, InventoryError> {
    let supports_da3 = server.supports_da3_browse();
    let supports_da2 = server.supports_da2_browse();
    if !supports_da3 && !supports_da2 {
        return Err(OpcError::NotImplemented(
            "Server exposes neither OPC DA 3.0 nor OPC DA 2.x browsing".to_string(),
        )
        .into());
    }
    let namespace = if supports_da2 {
        let organization = paced_call(
            boundary,
            InventoryNativeOperationKind::NamespaceOrganizationQuery,
            1,
            || server.query_organization(),
        )?;
        match organization {
            value if value == OPC_NS_FLAT.0.cast_unsigned() => {
                crate::provider::BrowseNamespace::Flat
            }
            value if value == crate::bindings::da::OPC_NS_HIERARCHIAL.0.cast_unsigned() => {
                crate::provider::BrowseNamespace::Hierarchical
            }
            value => {
                return Err(OpcError::Server(
                    "Server returned an unknown namespace organization".to_string(),
                    value,
                )
                .into());
            }
        }
    } else {
        crate::provider::BrowseNamespace::Unknown
    };
    Ok(BrowseCapabilities {
        namespace,
        supports_da3,
        supports_da2,
        max_page_size: crate::native_browse::MAX_BROWSE_PAGE_SIZE,
    })
}
