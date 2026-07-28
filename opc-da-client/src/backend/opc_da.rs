use crate::backend::connector::{ComConnector, ServerConnector};
use crate::com_worker::{ComRequest, ComWorker};
use crate::opc_da::errors::OpcResult;
use crate::provider::{OpcProvider, OpcValue, TagValue, WriteResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// Concrete [`OpcProvider`] implementation for Windows OPC DA.
///
/// Uses native `windows-rs` COM interop via the internal `opc_da` module.
pub struct OpcDaClient<C: ServerConnector + 'static = ComConnector> {
    pub worker: ComWorker<C>,
}

/// Returns the default `OpcDaClient` using native COM settings.
///
/// # Panics
///
/// Panics if the background COM worker thread cannot be started or COM
/// Multi-Threaded Apartment (MTA) initialization fails on the worker thread.
/// Use [`OpcDaClient::new`] for fallible construction.
impl Default for OpcDaClient<ComConnector> {
    fn default() -> Self {
        match Self::new(ComConnector) {
            Ok(client) => client,
            Err(err) => {
                tracing::error!(error = ?err, "Failed to initialize default OpcDaClient");
                Self {
                    worker: ComWorker::closed(),
                }
            }
        }
    }
}

impl<C: ServerConnector + 'static> OpcDaClient<C> {
    /// Creates a new `OpcDaClient` with the given connector.
    pub fn new(connector: C) -> OpcResult<Self> {
        tracing::info!("Initializing OpcDaClient...");
        let worker = ComWorker::start(Arc::new(connector))?;
        tracing::info!("OpcDaClient initialized successfully");
        Ok(Self { worker })
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl<C: ServerConnector + 'static> OpcProvider for OpcDaClient<C> {
    async fn list_servers(&self, host: &str) -> OpcResult<Vec<String>> {
        let host_owned = host.to_string();
        self.worker
            .send_request(|reply| ComRequest::ListServers {
                host: host_owned,
                reply,
            })
            .await
    }

    async fn browse_tags(
        &self,
        server: &str,
        max_tags: usize,
        progress: Arc<AtomicUsize>,
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
    ) -> OpcResult<Vec<String>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::BrowseTags {
                server: server_owned,
                max_tags,
                progress,
                tags_sink,
                reply,
            })
            .await
    }

    async fn read_tag_values(
        &self,
        server: &str,
        tag_ids: Vec<String>,
    ) -> OpcResult<Vec<TagValue>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: server_owned,
                tag_ids,
                reply,
            })
            .await
    }

    async fn write_tag_value(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
    ) -> OpcResult<WriteResult> {
        let server_owned = server.to_string();
        let tag_id_owned = tag_id.to_string();
        self.worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: server_owned,
                tag_id: tag_id_owned,
                value,
                reply,
            })
            .await
    }
}
