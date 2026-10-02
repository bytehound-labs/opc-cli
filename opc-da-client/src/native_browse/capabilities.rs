//! Native browse capability and namespace detection.

use crate::backend::connector::ConnectedServer;
use crate::bindings::da::{OPC_NS_FLAT, OPC_NS_HIERARCHIAL};
use crate::errors::{OpcError, OpcResult};
use crate::provider::{BrowseCapabilities, BrowseNamespace};
use std::time::Instant;

pub const MAX_BROWSE_PAGE_SIZE: u32 = 1_000;

pub fn capabilities_for_server<S: ConnectedServer>(server: &S) -> OpcResult<BrowseCapabilities> {
    let started = Instant::now();
    tracing::info!(target: "opc_da_client::native_browse", "native browse capability detection started");
    let supports_da3 = server.supports_da3_browse();
    let supports_da2 = server.supports_da2_browse();
    if !supports_da3 && !supports_da2 {
        return Err(OpcError::NotImplemented(
            "Server exposes neither OPC DA 3.0 nor OPC DA 2.x browsing".to_string(),
        ));
    }

    let namespace = if supports_da2 {
        let organization_started = Instant::now();
        match server.query_organization()? {
            value if value == OPC_NS_FLAT.0.cast_unsigned() => {
                tracing::info!(target: "opc_da_client::native_browse",
                    organization = "flat",
                    elapsed_ms = organization_started.elapsed().as_millis(),
                    "native namespace organization query completed"
                );
                BrowseNamespace::Flat
            }
            value if value == OPC_NS_HIERARCHIAL.0.cast_unsigned() => {
                tracing::info!(target: "opc_da_client::native_browse",
                    organization = "hierarchical",
                    elapsed_ms = organization_started.elapsed().as_millis(),
                    "native namespace organization query completed"
                );
                BrowseNamespace::Hierarchical
            }
            value => {
                return Err(OpcError::Server(
                    "Server returned an unknown namespace organization".to_string(),
                    value,
                ));
            }
        }
    } else {
        BrowseNamespace::Unknown
    };

    let capabilities = BrowseCapabilities {
        namespace,
        supports_da3,
        supports_da2,
        max_page_size: MAX_BROWSE_PAGE_SIZE,
    };
    tracing::info!(target: "opc_da_client::native_browse",
        supports_da2,
        supports_da3,
        namespace = ?capabilities.namespace,
        elapsed_ms = started.elapsed().as_millis(),
        "native browse capability detection completed"
    );
    Ok(capabilities)
}
