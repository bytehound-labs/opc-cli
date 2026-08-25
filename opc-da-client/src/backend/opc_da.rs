use crate::backend::connector::{ComConnector, ServerConnector};
use crate::com_worker::{ComRequest, ComWorker, ReadPresentation};
use crate::opc_da::errors::OpcResult;
use crate::provider::{
    BrowseCapabilities, BrowsePage, BrowsePageRequest, BrowseSessionToken, InventoryControl,
    InventoryOptions, InventoryStream, MAX_INVENTORY_BATCH_SIZE, OpcProvider, OpcValue, TagValue,
    WriteResult,
};
use async_trait::async_trait;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// Concrete [`OpcProvider`] implementation for Windows OPC DA.
///
/// Uses native `windows-rs` COM interop via the internal `opc_da` module.
pub struct OpcDaClient<C: ServerConnector + 'static = ComConnector> {
    pub worker: ComWorker<C>,
    connector: Arc<C>,
    inventory_active: Arc<AtomicBool>,
}

struct InventoryActiveGuard(Arc<AtomicBool>);

impl Drop for InventoryActiveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
                    connector: Arc::new(ComConnector),
                    inventory_active: Arc::new(AtomicBool::new(false)),
                }
            }
        }
    }
}

impl<C: ServerConnector + 'static> OpcDaClient<C> {
    /// Creates a new `OpcDaClient` with the given connector.
    pub fn new(connector: C) -> OpcResult<Self> {
        tracing::info!("Initializing OpcDaClient...");
        let connector = Arc::new(connector);
        let worker = ComWorker::start(Arc::clone(&connector))?;
        tracing::info!("OpcDaClient initialized successfully");
        Ok(Self {
            worker,
            connector,
            inventory_active: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn read_tag_values_with_presentation(
        &self,
        server: &str,
        tag_ids: Vec<String>,
        presentation: ReadPresentation,
    ) -> OpcResult<Vec<TagValue>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: server_owned,
                tag_ids,
                presentation,
                reply,
            })
            .await
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

    async fn browse_capabilities(&self, server: &str) -> OpcResult<BrowseCapabilities> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::BrowseCapabilities {
                server: server_owned,
                reply,
            })
            .await
    }

    async fn open_browse_session(&self, server: &str) -> OpcResult<BrowseSessionToken> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::OpenBrowseSession {
                server: server_owned,
                reply,
            })
            .await
    }

    async fn browse_page(
        &self,
        session: &BrowseSessionToken,
        request: BrowsePageRequest,
    ) -> OpcResult<BrowsePage> {
        let session = *session;
        self.worker
            .send_request(|reply| ComRequest::BrowsePage {
                session,
                request,
                reply,
            })
            .await
    }

    async fn close_browse_session(&self, session: &BrowseSessionToken) -> OpcResult<()> {
        let session = *session;
        self.worker
            .send_request(|reply| ComRequest::CloseBrowseSession { session, reply })
            .await
    }

    async fn start_inventory(
        &self,
        server: &str,
        options: InventoryOptions,
    ) -> OpcResult<InventoryStream> {
        if options.batch_size == 0 || options.batch_size > MAX_INVENTORY_BATCH_SIZE {
            return Err(crate::opc_da::errors::OpcError::InvalidState(format!(
                "Inventory batch size must be between 1 and {MAX_INVENTORY_BATCH_SIZE}"
            )));
        }
        if self.inventory_active.swap(true, Ordering::AcqRel) {
            return Err(crate::opc_da::errors::OpcError::InvalidState(
                "An OPC namespace inventory is already running".to_string(),
            ));
        }

        let (sender, receiver) = mpsc::channel(64);
        let control = InventoryControl::new_with_batch_size(options.batch_size);
        let worker_control = control.clone();
        let active = Arc::clone(&self.inventory_active);
        let connector = Arc::clone(&self.connector);
        let server = server.to_string();
        let spawn_result = std::thread::Builder::new()
            .name("opc-da-inventory".to_string())
            .spawn(move || {
                let _active_guard = InventoryActiveGuard(active);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let _guard = crate::ComGuard::new().map_err(|error| {
                        crate::opc_da::errors::OpcError::Internal(error.to_string())
                    })?;
                    crate::inventory::run_inventory(
                        &*connector,
                        &server,
                        options,
                        &worker_control,
                        &sender,
                    )
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::error!(server = %server, error = %error, "OPC namespace inventory failed");
                        if sender.blocking_send(Err(error)).is_err() {
                            tracing::debug!(
                                server = %server,
                                "OPC namespace inventory receiver was closed before failure could be delivered"
                            );
                        }
                    }
                    Err(payload) => {
                        let payload_type = panic_payload_type(&*payload);
                        tracing::error!(
                            server = %server,
                            payload_type,
                            "OPC namespace inventory worker panicked"
                        );
                        let error = crate::opc_da::errors::OpcError::Internal(
                            "OPC namespace inventory worker panicked".to_string(),
                        );
                        if sender.blocking_send(Err(error)).is_err() {
                            tracing::debug!(
                                server = %server,
                                "OPC namespace inventory receiver was closed before panic could be delivered"
                            );
                        }
                    }
                }
            });

        let worker = match spawn_result {
            Ok(worker) => worker,
            Err(error) => {
                self.inventory_active.store(false, Ordering::Release);
                return Err(crate::opc_da::errors::OpcError::Internal(format!(
                    "failed to start OPC inventory worker: {error}"
                )));
            }
        };

        Ok(InventoryStream::new(receiver, control, worker))
    }

    async fn read_tag_values(
        &self,
        server: &str,
        tag_ids: Vec<String>,
    ) -> OpcResult<Vec<TagValue>> {
        self.read_tag_values_with_presentation(server, tag_ids, ReadPresentation::Semantic)
            .await
    }

    async fn read_tag_values_for_display(
        &self,
        server: &str,
        tag_ids: Vec<String>,
    ) -> OpcResult<Vec<TagValue>> {
        self.read_tag_values_with_presentation(server, tag_ids, ReadPresentation::Display)
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

fn panic_payload_type(payload: &(dyn std::any::Any + Send)) -> &'static str {
    if payload.is::<&'static str>() {
        "&str"
    } else if payload.is::<String>() {
        "String"
    } else {
        "other"
    }
}
