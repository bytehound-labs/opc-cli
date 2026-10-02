//! Cached connection dispatch and classified one-retry eviction.

use crate::backend::connector::ServerConnector;
use crate::com_worker::ComWorker;
use crate::errors::{OpcError, OpcResult};
use std::collections::HashMap;
use std::sync::Arc;

#[allow(clippy::cast_possible_wrap)]
fn is_connection_error(err: &OpcError) -> bool {
    if let OpcError::Com { source } = err {
        let code = source.code().0;
        code == windows::core::HRESULT(0x8007_06BA_u32 as i32).0
            || code == windows::core::HRESULT(0x8007_06BF_u32 as i32).0
            || code == windows::core::HRESULT(0x8007_06BE_u32 as i32).0
            || code == windows::core::HRESULT(0x8008_0005_u32 as i32).0
    } else {
        false
    }
}

impl<C: ServerConnector + 'static> ComWorker<C> {
    pub(super) fn dispatch_with_retry<F, R>(
        cache: &mut HashMap<String, C::Server>,
        connector: &Arc<C>,
        server_name: &str,
        operation: F,
    ) -> OpcResult<R>
    where
        F: Fn(&C::Server) -> OpcResult<R>,
    {
        let server_ref = match cache.entry(server_name.to_string()) {
            std::collections::hash_map::Entry::Occupied(e) => {
                tracing::trace!(target: "opc_da_client::com_worker", server = %server_name, "Cache hit");
                e.into_mut()
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                tracing::debug!(target: "opc_da_client::com_worker", server = %server_name, "Cache miss, connecting");
                let srv = connector.connect(server_name)?;
                tracing::debug!(target: "opc_da_client::com_worker", server = %server_name, "Connection established, added to pool");
                e.insert(srv)
            }
        };

        match operation(server_ref) {
            Err(e) if is_connection_error(&e) => {
                tracing::warn!(target: "opc_da_client::com_worker", server = %server_name, error = ?e, "Evicting stale connection");
                cache.remove(server_name);
                tracing::debug!(target: "opc_da_client::com_worker", server = %server_name, "Reconnecting");
                let fresh_srv = connector.connect(server_name).map_err(|connect_e| {
                    tracing::error!(target: "opc_da_client::com_worker", error = ?connect_e, "Reconnect failed");
                    connect_e
                })?;
                let fresh_ref = &fresh_srv;
                let result = operation(fresh_ref);
                tracing::debug!(target: "opc_da_client::com_worker", server = %server_name, "Reconnection successful, pool updated");
                cache.insert(server_name.to_string(), fresh_srv);
                result
            }
            other => other,
        }
    }
}
