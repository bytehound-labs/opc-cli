//! Dedicated MTA worker lifecycle, bounded request channel, and result delivery.

mod browse;
mod connection;
mod read;
mod request;
mod write;

pub use request::{ComRequest, ReadPresentation};

use crate::backend::connector::ServerConnector;
use crate::errors::{OpcError, OpcResult};
use crate::native_browse::{BrowseSessions, capabilities_for_server};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Dedicated background worker thread manager handling COM MTA apartment thread affinity.
///
/// Dispatches requests received over an `mpsc` channel to Windows COM interfaces while maintaining
/// a persistent connection pool and transparently evicting stale connection handles on RPC errors.
pub struct ComWorker<C: ServerConnector + 'static> {
    /// Channel sender for dispatching requests to the worker loop.
    pub sender: mpsc::Sender<ComRequest>,
    /// Join handle for explicit teardown after the request channel is closed.
    pub handle: Option<std::thread::JoinHandle<()>>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: ServerConnector + 'static> ComWorker<C> {
    /// Creates a dummy/closed `ComWorker` handle used when background worker initialization fails.
    pub fn closed() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        Self {
            sender: tx,
            handle: None,
            _phantom: std::marker::PhantomData,
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(connector))]
    pub fn start(connector: Arc<C>) -> Result<Self, OpcError> {
        let (tx, mut rx) = mpsc::channel(32);
        let (init_tx, init_rx) = std::sync::mpsc::channel();

        let handle = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            tracing::info!(
                thread_id = ?std::thread::current().id(),
                "COM worker thread started; initializing MTA"
            );
            let _guard = match crate::ComGuard::new() {
                Ok(g) => {
                    tracing::info!(
                        thread_id = ?std::thread::current().id(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "COM worker MTA initialized"
                    );
                    let _ = init_tx.send(Ok(()));
                    g
                }
                Err(e) => {
                    tracing::error!(error = ?e, "COM worker failed to initialize MTA");
                    let _ =
                        init_tx.send(Err(OpcError::Internal("COM init failed on worker".into())));
                    return;
                }
            };

            let mut cache: HashMap<String, C::Server> = HashMap::new();
            let mut browse_sessions = BrowseSessions::default();

            while let Some(req) = rx.blocking_recv() {
                browse_sessions.cleanup_expired();
                match req {
                    ComRequest::ListServers { host, reply } => {
                        let span = tracing::info_span!("opc.list_servers", host = %host);
                        let _enter = span.enter();
                        #[cfg(feature = "dev-diagnostics")]
                        tracing::trace!(host = %host, "list_servers: starting operation");
                        let start = std::time::Instant::now();
                        let servers = connector.enumerate_servers();
                        if let Ok(s) = &servers {
                            tracing::debug!(
                                count = s.len(),
                                elapsed_ms =
                                    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                                "list_servers completed"
                            );
                        } else if let Err(e) = &servers {
                            crate::opc_da::errors::log_opc_error(e, "list_servers");
                            tracing::error!(
                                error = ?e,
                                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                                "list_servers failed"
                            );
                        }
                        let _ = reply.send(servers);
                    }

                    ComRequest::ReadTagValues {
                        server,
                        tag_ids,
                        presentation,
                        reply,
                    } => {
                        let result = Self::dispatch_with_retry(
                            &mut cache,
                            &connector,
                            &server,
                            |opc_server| {
                                Self::handle_read(&server, &tag_ids, presentation, opc_server)
                            },
                        );
                        let _ = reply.send(result);
                    }
                    ComRequest::WriteTagValue {
                        server,
                        tag_id,
                        value,
                        reply,
                    } => {
                        let result = Self::dispatch_with_retry(
                            &mut cache,
                            &connector,
                            &server,
                            |opc_server| Self::handle_write(&server, &tag_id, &value, opc_server),
                        );
                        let _ = reply.send(result);
                    }
                    ComRequest::BrowseTags {
                        server,
                        max_tags,
                        progress,
                        tags_sink,
                        reply,
                    } => {
                        let result = Self::dispatch_with_retry(
                            &mut cache,
                            &connector,
                            &server,
                            |opc_server| {
                                Self::handle_browse(
                                    &server, max_tags, &progress, &tags_sink, opc_server,
                                )
                            },
                        );
                        let _ = reply.send(result);
                    }
                    ComRequest::BrowseCapabilities { server, reply } => {
                        if reply.is_closed() {
                            continue;
                        }
                        let result = Self::dispatch_with_retry(
                            &mut cache,
                            &connector,
                            &server,
                            capabilities_for_server,
                        );
                        let _ = reply.send(result);
                    }
                    ComRequest::OpenBrowseSession { server, reply } => {
                        if reply.is_closed() {
                            continue;
                        }
                        let result = connector
                            .connect(&server)
                            .and_then(|opc_server| browse_sessions.open(opc_server));
                        if let Err(Ok(session)) = reply.send(result) {
                            let _ = browse_sessions.close(&session);
                        }
                    }
                    ComRequest::BrowsePage {
                        session,
                        request,
                        reply,
                    } => {
                        if reply.is_closed() {
                            let _ = browse_sessions.close(&session);
                            continue;
                        }
                        let result = browse_sessions.page(&session, request);
                        if reply.send(result).is_err() {
                            let _ = browse_sessions.close(&session);
                        }
                    }
                    ComRequest::CloseBrowseSession { session, reply } => {
                        let result = browse_sessions.close(&session);
                        let _ = reply.send(result);
                    }
                }
            }

            tracing::debug!("COM worker thread exiting cleanly");
        });

        init_rx
            .recv()
            .map_err(|_| OpcError::Internal("COM worker thread panicked during init".into()))??;

        tracing::debug!("COM worker thread started");

        Ok(Self {
            sender: tx,
            handle: Some(handle),
            _phantom: std::marker::PhantomData,
        })
    }

    #[tracing::instrument(skip(self, req_builder))]
    pub async fn send_request<F, R>(&self, req_builder: F) -> OpcResult<R>
    where
        F: FnOnce(oneshot::Sender<OpcResult<R>>) -> ComRequest,
    {
        if self
            .handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            tracing::error!("COM worker thread panicked or exited unexpectedly");
            return Err(OpcError::Internal("COM worker thread panicked".into()));
        }

        let (tx, rx) = oneshot::channel();
        let req = req_builder(tx);

        self.sender
            .send(req)
            .await
            .map_err(|_| OpcError::Internal("COM worker channel closed (worker stopped)".into()))?;

        rx.await
            .map_err(|_| OpcError::Internal("COM worker shut down during request".into()))?
    }
}

impl<C: ServerConnector + 'static> Drop for ComWorker<C> {
    fn drop(&mut self) {
        tracing::debug!("ComWorker dropping — channel closing, signaling thread shutdown");
    }
}

#[cfg(test)]
#[path = "tests/com_worker.rs"]
mod tests;
