use crate::backend::connector::{
    ConnectedGroup, ConnectedServer, ServerConnector, guard_browse_iterator,
};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_DS_DEVICE, OPC_LEAF, OPC_NS_FLAT, tagOPCITEMDEF,
};
use crate::helpers::{
    filetime_to_string, format_hresult, opc_value_to_variant, quality_to_string,
    variant_to_display_string, variant_to_string,
};
use crate::native_browse::{BrowseSessions, capabilities_for_server};
use crate::opc_da::errors::{
    OpcError, OpcResult, contextual_browse_error, is_non_progress_browse_error,
};
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use crate::provider::{
    BrowseCapabilities, BrowsePage, BrowsePageRequest, BrowseSessionToken, OpcValue, TagValue,
    WriteResult,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, oneshot};

/// Controls whether read values preserve machine semantics or use TUI-oriented display formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPresentation {
    /// Return `VT_BSTR` contents exactly as stored by COM.
    Semantic,
    /// Wrap `VT_BSTR` contents in quotes for human-readable display.
    Display,
}

/// Represents a asynchronous request dispatched to the COM worker thread.
pub enum ComRequest {
    /// Request to enumerate available OPC DA servers on a host.
    ListServers {
        /// Hostname or IP address to target.
        host: String,
        /// One-shot channel to send back the server enumeration result.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request to read current values, quality, and timestamps for tag IDs.
    ReadTagValues {
        /// OPC server ProgID.
        server: String,
        /// List of fully qualified tag identifiers to read.
        tag_ids: Vec<String>,
        /// Value formatting intent for this read.
        presentation: ReadPresentation,
        /// One-shot channel to send back the tag values result.
        reply: oneshot::Sender<OpcResult<Vec<TagValue>>>,
    },
    /// Request to write a typed value to a single tag.
    WriteTagValue {
        /// OPC server ProgID.
        server: String,
        /// Tag identifier to write.
        tag_id: String,
        /// Typed value to write.
        value: OpcValue,
        /// One-shot channel to send back the write operation result.
        reply: oneshot::Sender<OpcResult<WriteResult>>,
    },
    /// Request to recursively browse available tags on a server.
    BrowseTags {
        /// OPC server ProgID.
        server: String,
        /// Maximum number of tags to discover before stopping.
        max_tags: usize,
        /// Atomic counter tracking total tags discovered.
        progress: Arc<AtomicUsize>,
        /// Shared mutex-protected vector storing discovered tag names incrementally.
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        /// One-shot channel to send back the complete tag discovery list.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request the native browse capabilities of a server.
    BrowseCapabilities {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the capabilities.
        reply: oneshot::Sender<OpcResult<BrowseCapabilities>>,
    },
    /// Open an isolated native browse session.
    OpenBrowseSession {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the opaque session token.
        reply: oneshot::Sender<OpcResult<BrowseSessionToken>>,
    },
    /// Request one bounded native browse page.
    BrowsePage {
        /// Opaque browse session token.
        session: BrowseSessionToken,
        /// One-level browse request.
        request: BrowsePageRequest,
        /// One-shot channel to send back the page.
        reply: oneshot::Sender<OpcResult<BrowsePage>>,
    },
    /// Close an isolated native browse session.
    CloseBrowseSession {
        /// Opaque browse session token.
        session: BrowseSessionToken,
        /// One-shot channel to report completion.
        reply: oneshot::Sender<OpcResult<()>>,
    },
}

/// Dedicated background worker thread manager handling COM MTA apartment thread affinity.
///
/// Dispatches requests received over an `mpsc` channel to Windows COM interfaces while maintaining
/// a persistent connection pool and transparently evicting stale connection handles on RPC errors.
pub struct ComWorker<C: ServerConnector + 'static> {
    /// Channel sender for dispatching requests to the worker loop.
    pub sender: mpsc::Sender<ComRequest>,
    /// Thread join handle for clean worker thread teardown.
    pub handle: Option<std::thread::JoinHandle<()>>,
    _phantom: std::marker::PhantomData<C>,
}

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
            tracing::debug!("COM worker thread spawned, initializing COM (MTA)");
            let _guard = match crate::ComGuard::new() {
                Ok(g) => {
                    tracing::info!("COM MTA initialized successfully on worker thread");
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

    fn dispatch_with_retry<F, R>(
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
                tracing::trace!(server = %server_name, "Cache hit");
                e.into_mut()
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                tracing::debug!(server = %server_name, "Cache miss, connecting");
                let srv = connector.connect(server_name)?;
                tracing::debug!(server = %server_name, "Connection established, added to pool");
                e.insert(srv)
            }
        };

        match operation(server_ref) {
            Err(e) if is_connection_error(&e) => {
                tracing::warn!(server = %server_name, error = ?e, "Evicting stale connection");
                cache.remove(server_name);
                tracing::debug!(server = %server_name, "Reconnecting");
                let fresh_srv = connector.connect(server_name).map_err(|connect_e| {
                    tracing::error!(error = ?connect_e, "Reconnect failed");
                    connect_e
                })?;
                let fresh_ref = &fresh_srv;
                let result = operation(fresh_ref);
                tracing::debug!(server = %server_name, "Reconnection successful, pool updated");
                cache.insert(server_name.to_string(), fresh_srv);
                result
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_read(
        server_name: &str,
        tag_ids: &[String],
        presentation: ReadPresentation,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<TagValue>> {
        let span = tracing::info_span!(
            "opc.read_tag_values",
            server = %server_name,
            tag_count = tag_ids.len()
        );
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(
            server = %server_name,
            tag_count = tag_ids.len(),
            sample_tags = ?tag_ids.iter().take(5).collect::<Vec<_>>(),
            "read_tag_values: starting operation"
        );
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-read",
            true,
            1000,
            server_handle,
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = tag_ids
            .iter()
            .map(|tag_id| tag_id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();

        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;

        // RemoteArray::len() returns u32; tag_ids.len() returns usize.
        if results.len() as usize != tag_ids.len() || errors.len() as usize != tag_ids.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        let mut tag_values: Vec<TagValue> = tag_ids
            .iter()
            .map(|tag_id| TagValue {
                tag_id: tag_id.clone(),
                value: "Error".to_string(),
                quality: "Bad — not added to group".to_string(),
                timestamp: String::new(),
            })
            .collect();

        let mut server_handles: Vec<ItemHandle> = Vec::new();
        let mut valid_indices = Vec::new();

        for (idx, (item_result, error)) in results
            .as_slice()
            .iter()
            .zip(errors.as_slice().iter())
            .enumerate()
        {
            if error.is_ok() {
                server_handles.push(ItemHandle(item_result.hServer));
                valid_indices.push(idx);
            } else {
                let hint = format_hresult(*error);
                tracing::warn!(
                    tag = %tag_ids[idx],
                    error = %hint,
                    "read_tag_values: add_items rejected tag"
                );
                tag_values[idx].quality = format!("Bad — {hint}");
            }
        }

        if server_handles.is_empty() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Ok(tag_values);
        }

        let (item_states, read_errors) = group.read(OPC_DS_DEVICE, &server_handles)?;
        let item_states_slice = item_states.as_slice();
        let read_errors_slice = read_errors.as_slice();

        for (i, idx) in valid_indices.iter().enumerate() {
            let state = &item_states_slice[i];
            let read_error = &read_errors_slice[i];

            let (value_str, quality_str) = if read_error.is_ok() {
                (
                    match presentation {
                        ReadPresentation::Semantic => variant_to_string(&state.vDataValue),
                        ReadPresentation::Display => variant_to_display_string(&state.vDataValue),
                    },
                    quality_to_string(state.wQuality),
                )
            } else {
                let full_msg = format_hresult(*read_error);
                tracing::warn!(
                    tag = %tag_ids[*idx],
                    error = ?read_error,
                    hint = %full_msg,
                    "read_tag_values: per-item read error"
                );
                ("Error".to_string(), format!("Bad — {full_msg}"))
            };

            tag_values[*idx] = TagValue {
                tag_id: tag_ids[*idx].clone(),
                value: value_str,
                quality: quality_str,
                timestamp: filetime_to_string(state.ftTimeStamp),
            };
        }

        tracing::debug!(
            count = tag_values.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "read_tag_values completed"
        );
        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
        }
        Ok(tag_values)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_write(
        server_name: &str,
        tag_id: &str,
        value: &OpcValue,
        opc_server: &C::Server,
    ) -> OpcResult<WriteResult> {
        let span = tracing::info_span!(
            "opc.write_tag_value",
            server = %server_name,
            tag = %tag_id
        );
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(
            server = %server_name,
            tag = %tag_id,
            value = ?value,
            "write_tag_value: starting operation"
        );
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-write",
            true,
            1000,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let mut item_id_wide: Vec<u16> = tag_id.encode_utf16().chain(std::iter::once(0)).collect();
        let item_def = tagOPCITEMDEF {
            szAccessPath: windows::core::PWSTR::null(),
            szItemID: windows::core::PWSTR(item_id_wide.as_mut_ptr()),
            bActive: windows::Win32::Foundation::TRUE,
            hClient: 0,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
            vtRequestedDataType: 0,
            wReserved: 0,
        };

        let (results, errors) = group.add_items(&[item_def])?;
        let item_res = results
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item results".to_string()))?;
        let item_err = errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item errors".to_string()))?;

        if let Err(e) = item_err.ok() {
            tracing::warn!(error = ?e, "write_tag_value: failed to add tag to group");
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
            }
            return Ok(WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(format!("Failed to add tag: {}", format_hresult(*item_err))),
            });
        }

        let item_handle = ItemHandle(item_res.hServer);
        let variant = opc_value_to_variant(value);

        let write_errors = group.write(&[item_handle], &[variant])?;
        let write_err = write_errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty write errors".to_string()))?;

        let write_result = if write_err.is_ok() {
            tracing::debug!(
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value completed"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: true,
                error: None,
            }
        } else {
            let msg = format_hresult(*write_err);
            tracing::warn!(
                error = %msg,
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value: server rejected write"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(msg),
            }
        };

        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
        }
        Ok(write_result)
    }

    fn handle_browse(
        server_name: &str,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<String>> {
        let span = tracing::info_span!("opc.browse_tags", server = %server_name, max_tags);
        let _enter = span.enter();
        #[cfg(feature = "dev-diagnostics")]
        tracing::trace!(
            server = %server_name,
            max_tags,
            "browse_tags: starting operation"
        );
        let start = std::time::Instant::now();

        let org = opc_server.query_organization()?;
        let mut tags = Vec::new();

        if org == OPC_NS_FLAT.0 as u32 {
            let mut string_iter = guard_browse_iterator(
                opc_server.begin_da2_browse(OPC_LEAF.0 as u32, Some(""), 0, 0)?,
                "recursive flat iterator",
                &[],
            );
            while let Some(tag_res) = string_iter.next_string() {
                if tags.len() >= max_tags {
                    break;
                }
                let tag = tag_res?;
                tags.push(tag.clone());
                if let Ok(mut sink) = tags_sink.lock() {
                    sink.push(tag);
                }
                progress.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            Self::browse_recursive(
                opc_server,
                &mut tags,
                max_tags,
                progress,
                tags_sink,
                &mut Vec::new(),
                0,
            )?;
        }
        tracing::debug!(
            count = tags.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "browse_tags completed"
        );
        Ok(tags)
    }

    fn browse_recursive(
        server: &C::Server,
        tags: &mut Vec<String>,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        browse_path: &mut Vec<String>,
        depth: usize,
    ) -> OpcResult<()> {
        const MAX_DEPTH: usize = 50;
        if depth > MAX_DEPTH || tags.len() >= max_tags {
            if depth > MAX_DEPTH {
                tracing::warn!(depth, "Max browse depth reached, truncating");
            }
            return Ok(());
        }

        let mut branch_enum = guard_browse_iterator(
            server.begin_da2_browse(OPC_BRANCH.0 as u32, Some(""), 0, 0)?,
            "recursive branch iterator",
            browse_path,
        );
        let mut branches = Vec::new();
        while let Some(result) = branch_enum.next_string() {
            match result {
                Ok(name) => branches.push(name),
                Err(error) if is_non_progress_browse_error(&error) => {
                    return Err(contextual_browse_error(
                        error,
                        "browse_recursive(branches)",
                        browse_path,
                        None,
                    ));
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "Branch iteration error, skipping");
                }
            }
        }

        let mut leaf_enum = guard_browse_iterator(
            server.begin_da2_browse(OPC_LEAF.0 as u32, Some(""), 0, 0)?,
            "recursive leaf iterator",
            browse_path,
        );
        while let Some(tag_res) = leaf_enum.next_string() {
            if tags.len() >= max_tags {
                return Ok(());
            }
            let browse_name = tag_res?;
            let tag = match server.get_item_id(&browse_name) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        browse_name = %browse_name,
                        error = ?e,
                        "get_item_id failed, using browse name as fallback"
                    );
                    browse_name
                }
            };
            tags.push(tag.clone());
            if let Ok(mut sink) = tags_sink.lock() {
                sink.push(tag);
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }

        for branch in branches {
            if tags.len() >= max_tags {
                return Ok(());
            }
            if let Err(e) = server.change_browse_position(OPC_BROWSE_DOWN.0 as u32, &branch) {
                tracing::warn!(
                    branch = %branch,
                    error = ?e,
                    "Failed to browse down, skipping branch"
                );
                continue;
            }

            browse_path.push(branch.clone());
            let recurse_result = Self::browse_recursive(
                server,
                tags,
                max_tags,
                progress,
                tags_sink,
                browse_path,
                depth + 1,
            );

            let up_result = server.change_browse_position(OPC_BROWSE_UP.0 as u32, "");
            browse_path.pop();

            if let Err(e) = recurse_result {
                if is_non_progress_browse_error(&e) {
                    return Err(e);
                }
                tracing::warn!(error = ?e, "browse_recursive error");
            }

            if let Err(e) = up_result {
                tracing::warn!(error = ?e, "Failed to browse up, stopping recursion");
                break;
            }
        }

        Ok(())
    }
}

impl<C: ServerConnector + 'static> Drop for ComWorker<C> {
    fn drop(&mut self) {
        tracing::debug!("ComWorker dropping — channel closing, signaling thread shutdown");
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::single_char_pattern,
        clippy::cast_possible_wrap,
        clippy::ptr_as_ptr,
        clippy::borrow_as_ptr,
        clippy::mixed_attributes_style,
        clippy::unreadable_literal,
        clippy::undocumented_unsafe_blocks,
        clippy::manual_assert
    )]
    use super::*;
    use crate::backend::connector::{
        BrowseStringIterator, ConnectedGroup, ConnectedServer, RemoteArray, ServerConnector,
        StringIterator,
    };
    use crate::bindings::da::OPC_FLAT;
    use crate::bindings::da::{tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE};
    use crate::provider::BrowseNodeFilter;

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct MockState {
        connect_count: AtomicUsize,
        should_fail_connect: AtomicBool,
        should_fail_write: AtomicBool,
        should_fail_with_connection_error: AtomicBool,
        should_panic_on_request: AtomicBool,
        read_value: Mutex<String>,
    }

    struct ConfigurableMockConnector {
        state: Arc<MockState>,
    }

    struct ConfigurableMockServer {
        state: Arc<MockState>,
    }

    struct ConfigurableMockGroup {
        state: Arc<MockState>,
    }

    impl ConnectedGroup for ConfigurableMockGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            use windows::Win32::Foundation::S_OK;

            let res = tagOPCITEMRESULT {
                hServer: 1,
                vtCanonicalDataType: 0,
                wReserved: 0,
                dwAccessRights: 1,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
            };

            let res_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<tagOPCITEMRESULT>())
            } as *mut tagOPCITEMRESULT;
            unsafe {
                std::ptr::write(res_ptr, res);
            }
            let res_array = RemoteArray::from_mut_ptr(res_ptr, 1);

            let err_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<
                    windows::core::HRESULT,
                >())
            } as *mut windows::core::HRESULT;
            unsafe {
                std::ptr::write(err_ptr, S_OK);
            }
            let err_array = RemoteArray::from_mut_ptr(err_ptr, 1);

            Ok((res_array, err_array))
        }

        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            use windows::Win32::Foundation::S_OK;

            let value = self.state.read_value.lock().unwrap().clone();
            let item_state = tagOPCITEMSTATE {
                hClient: 0,
                ftTimeStamp: windows::Win32::Foundation::FILETIME::default(),
                wQuality: 0xC0,
                wReserved: 0,
                vDataValue: opc_value_to_variant(&OpcValue::String(value)),
            };
            let state_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<tagOPCITEMSTATE>())
            } as *mut tagOPCITEMSTATE;
            unsafe {
                std::ptr::write(state_ptr, item_state);
            }

            let error_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<
                    windows::core::HRESULT,
                >())
            } as *mut windows::core::HRESULT;
            unsafe {
                std::ptr::write(error_ptr, S_OK);
            }

            Ok((
                RemoteArray::from_mut_ptr(state_ptr, 1),
                RemoteArray::from_mut_ptr(error_ptr, 1),
            ))
        }

        fn write(
            &self,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            if self
                .state
                .should_fail_with_connection_error
                .load(Ordering::Relaxed)
            {
                // RPC server unavailable (0x800706BA) triggers connection eviction
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x800706BA_u32 as i32,
                    )),
                });
            }

            let hr = if self.state.should_fail_write.load(Ordering::Relaxed) {
                windows::Win32::Foundation::E_FAIL
            } else {
                windows::Win32::Foundation::S_OK
            };

            let hr_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<
                    windows::core::HRESULT,
                >())
            } as *mut windows::core::HRESULT;
            unsafe {
                std::ptr::write(hr_ptr, hr);
            }

            Ok(RemoteArray::from_mut_ptr(hr_ptr, 1))
        }
    }

    impl ConnectedServer for ConfigurableMockServer {
        type Group = ConfigurableMockGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(0)
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Ok(String::new())
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            if self.state.should_panic_on_request.load(Ordering::Relaxed) {
                panic!("Simulated worker panic");
            }
            Ok(ConfigurableMockGroup {
                state: self.state.clone(),
            })
        }

        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            Ok(())
        }
    }

    impl ServerConnector for ConfigurableMockConnector {
        type Server = ConfigurableMockServer;

        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            if self.state.should_fail_connect.load(Ordering::Relaxed) {
                Err(OpcError::Internal("Server enumeration failed".into()))
            } else {
                Ok(vec!["Mock.Server.1".into()])
            }
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            if self.state.should_fail_connect.load(Ordering::Relaxed) {
                Err(OpcError::Internal("Connection failed".into()))
            } else {
                self.state.connect_count.fetch_add(1, Ordering::Relaxed);
                Ok(ConfigurableMockServer {
                    state: self.state.clone(),
                })
            }
        }
    }

    struct WorkerMockConnector;
    struct WorkerMockServer;
    struct WorkerMockGroup;

    impl ConnectedGroup for WorkerMockGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn write(
            &self,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            Err(OpcError::NotImplemented("mock".into()))
        }
    }

    impl ConnectedServer for WorkerMockServer {
        type Group = WorkerMockGroup;
        fn query_organization(&self) -> OpcResult<u32> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".into()))
        }
    }

    impl ServerConnector for WorkerMockConnector {
        type Server = WorkerMockServer;
        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(vec!["Mock.Server.1".into()])
        }
        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(WorkerMockServer)
        }
    }

    #[tokio::test]
    async fn test_worker_starts_and_stops() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(WorkerMockConnector)).unwrap()
        })
        .await
        .unwrap();
        drop(worker);
    }

    #[tokio::test]
    async fn test_worker_list_servers() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(WorkerMockConnector)).unwrap()
        })
        .await
        .unwrap();
        let (reply, _rx) = oneshot::channel();
        worker
            .sender
            .send(ComRequest::ListServers {
                host: "localhost".into(),
                reply,
            })
            .await
            .unwrap();
        // Wait for implementation
    }

    struct MismatchedConnector;
    struct MismatchedServer;
    struct MismatchedGroup;

    impl ConnectedGroup for MismatchedGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Ok((RemoteArray::empty(), RemoteArray::empty()))
        }
        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Ok((RemoteArray::empty(), RemoteArray::empty()))
        }
        fn write(
            &self,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            Ok(RemoteArray::empty())
        }
    }

    impl ConnectedServer for MismatchedServer {
        type Group = MismatchedGroup;
        fn query_organization(&self) -> OpcResult<u32> {
            Ok(0)
        }
        fn browse_opc_item_ids(
            &self,
            _b: u32,
            _f: Option<&str>,
            _d: u16,
            _a: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }
        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Ok(String::new())
        }
        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            Ok(MismatchedGroup)
        }
        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            Ok(())
        }
    }

    impl ServerConnector for MismatchedConnector {
        type Server = MismatchedServer;
        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(vec![])
        }
        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(MismatchedServer)
        }
    }

    #[tokio::test]
    async fn test_worker_read_tag_values_mismatched_lengths() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(MismatchedConnector)).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: "MockServer".to_string(),
                tag_ids: vec!["Tag1".to_string(), "Tag2".to_string()],
                presentation: ReadPresentation::Semantic,
                reply,
            })
            .await;

        assert!(
            result.is_err(),
            "Expected read to fail due to mismatched lengths"
        );
        if let Err(OpcError::Internal(msg)) = result {
            assert!(msg.contains("mismatched result array sizes"));
        } else {
            panic!("Expected OpcError::Internal, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_worker_routes_read_presentation() {
        let state = Arc::new(MockState {
            read_value: Mutex::new("AUT".to_string()),
            ..MockState::default()
        });
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let semantic = worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["StringTag".to_string()],
                presentation: ReadPresentation::Semantic,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(semantic[0].value, "AUT");

        let display = worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["StringTag".to_string()],
                presentation: ReadPresentation::Display,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(display[0].value, "\"AUT\"");
    }

    #[tokio::test]
    async fn test_worker_write_tag_value() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Random.Int4".to_string(),
                value: OpcValue::Int(42),
                reply,
            })
            .await
            .expect("Request should succeed");

        assert_eq!(result.tag_id, "Random.Int4");
        assert!(result.success, "Write should be successful");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_connection_cache_reuse() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await
            .unwrap();

        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag2".to_string(),
                value: OpcValue::Int(2),
                reply,
            })
            .await
            .unwrap();

        assert_eq!(
            state.connect_count.load(Ordering::Relaxed),
            1,
            "Server connection should be cached and reused"
        );
    }

    #[tokio::test]
    async fn test_stale_connection_eviction() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // Initial connect
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await
            .unwrap();

        assert_eq!(state.connect_count.load(Ordering::Relaxed), 1);

        // Enable connection error flag to trigger eviction on next operation
        state
            .should_fail_with_connection_error
            .store(true, Ordering::Relaxed);

        // Next request triggers eviction and reconnect attempt
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag2".to_string(),
                value: OpcValue::Int(2),
                reply,
            })
            .await;

        assert_eq!(
            state.connect_count.load(Ordering::Relaxed),
            2,
            "Stale connection should be evicted and reconnected"
        );
    }

    #[tokio::test]
    async fn test_worker_panic_propagation() {
        let state = Arc::new(MockState::default());
        state.should_panic_on_request.store(true, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await;

        assert!(result.is_err());
        if let Err(OpcError::Internal(msg)) = result {
            assert!(
                msg.contains("shut down")
                    || msg.contains("channel closed")
                    || msg.contains("panicked"),
                "Expected worker termination message, got: {}",
                msg
            );
        } else {
            panic!("Expected OpcError::Internal, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_drop_during_active_request() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // Dropping worker handle closes channel gracefully
        drop(worker);
    }

    #[tokio::test]
    async fn test_worker_init_failure() {
        struct FailingInitConnector;
        impl ServerConnector for FailingInitConnector {
            type Server = ConfigurableMockServer;
            fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
                Err(OpcError::Internal("COM subsystem failed".into()))
            }
            fn connect(&self, _name: &str) -> OpcResult<Self::Server> {
                Err(OpcError::Internal("COM subsystem failed".into()))
            }
        }

        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(FailingInitConnector)).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::ListServers {
                host: "localhost".into(),
                reply,
            })
            .await;

        assert!(
            result.is_err(),
            "ListServers request should fail when connector enumeration fails"
        );
    }

    #[derive(Default)]
    struct BranchOnlyFlatState {
        flat_calls: AtomicUsize,
        position: Mutex<Vec<String>>,
    }

    struct BranchOnlyFlatConnector {
        state: Arc<BranchOnlyFlatState>,
    }

    struct BranchOnlyFlatServer {
        state: Arc<BranchOnlyFlatState>,
    }

    impl ConnectedServer for BranchOnlyFlatServer {
        type Group = WorkerMockGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(crate::bindings::da::OPC_NS_HIERARCHIAL.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn begin_da2_browse(
            &self,
            browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            let position = self.state.position.lock().unwrap();
            let values = if browse_type == OPC_FLAT.0.cast_unsigned() {
                self.state.flat_calls.fetch_add(1, Ordering::Relaxed);
                vec!["Area".to_string()]
            } else if browse_type == OPC_BRANCH.0.cast_unsigned() && position.is_empty() {
                vec!["Area".to_string()]
            } else if browse_type == OPC_LEAF.0.cast_unsigned() && position.as_slice() == ["Area"] {
                vec!["Tag".to_string()]
            } else {
                vec![]
            };
            Ok(Box::new(values.into_iter().map(Ok)))
        }

        fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
            let mut position = self.state.position.lock().unwrap();
            if direction == OPC_BROWSE_DOWN.0.cast_unsigned() {
                position.push(name.to_string());
            } else if direction == OPC_BROWSE_UP.0.cast_unsigned() {
                position.pop();
            }
            drop(position);
            Ok(())
        }

        fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
            let position = self.state.position.lock().unwrap();
            let item_id = format!("{}.{}", position.join("."), item_name);
            drop(position);
            Ok(item_id)
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }
    }

    impl ServerConnector for BranchOnlyFlatConnector {
        type Server = BranchOnlyFlatServer;

        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(vec![])
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(BranchOnlyFlatServer {
                state: self.state.clone(),
            })
        }
    }

    #[tokio::test]
    async fn hierarchical_browse_does_not_treat_branch_only_opc_flat_as_items() {
        let state = Arc::new(BranchOnlyFlatState::default());
        let connector = Arc::new(BranchOnlyFlatConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::BrowseTags {
                server: "Mock.Server".to_string(),
                max_tags: 10,
                progress: Arc::new(AtomicUsize::new(0)),
                tags_sink: Arc::new(Mutex::new(Vec::new())),
                reply,
            })
            .await
            .unwrap();

        assert_eq!(result, vec!["Area.Tag"]);
        assert_eq!(state.flat_calls.load(Ordering::Relaxed), 0);
    }

    #[derive(Default)]
    struct CancelledBrowseState {
        connect_count: AtomicUsize,
        drop_count: AtomicUsize,
    }

    struct CancelledBrowseConnector {
        state: Arc<CancelledBrowseState>,
    }

    struct CancelledBrowseServer {
        state: Arc<CancelledBrowseState>,
    }

    impl Drop for CancelledBrowseServer {
        fn drop(&mut self) {
            self.state.drop_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl ConnectedServer for CancelledBrowseServer {
        type Group = WorkerMockGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(OPC_NS_FLAT.0.cast_unsigned())
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn begin_da2_browse(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<Box<dyn BrowseStringIterator>> {
            Ok(Box::new(std::iter::empty()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("mock".to_string()))
        }

        fn remove_group(&self, _server_group: GroupHandle, _force: bool) -> OpcResult<()> {
            Ok(())
        }
    }

    impl ServerConnector for CancelledBrowseConnector {
        type Server = CancelledBrowseServer;

        fn enumerate_servers(&self) -> OpcResult<Vec<String>> {
            Ok(vec![])
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            self.state.connect_count.fetch_add(1, Ordering::Relaxed);
            Ok(CancelledBrowseServer {
                state: self.state.clone(),
            })
        }
    }

    #[tokio::test]
    async fn cancelled_native_browse_requests_release_or_avoid_sessions() {
        let state = Arc::new(CancelledBrowseState::default());
        let connector = Arc::new(CancelledBrowseConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let session = worker
            .send_request(|reply| ComRequest::OpenBrowseSession {
                server: "Mock.Server".to_string(),
                reply,
            })
            .await
            .unwrap();
        assert_eq!(state.connect_count.load(Ordering::Relaxed), 1);

        let (page_reply, page_receiver) = oneshot::channel();
        drop(page_receiver);
        worker
            .sender
            .send(ComRequest::BrowsePage {
                session,
                request: BrowsePageRequest {
                    parent: None,
                    filter: BrowseNodeFilter::All,
                    max_elements: 10,
                    continuation: None,
                },
                reply: page_reply,
            })
            .await
            .unwrap();
        worker
            .send_request(|reply| ComRequest::ListServers {
                host: "localhost".to_string(),
                reply,
            })
            .await
            .unwrap();
        assert_eq!(state.drop_count.load(Ordering::Relaxed), 1);

        let (open_reply, open_receiver) = oneshot::channel();
        drop(open_receiver);
        worker
            .sender
            .send(ComRequest::OpenBrowseSession {
                server: "Mock.Server".to_string(),
                reply: open_reply,
            })
            .await
            .unwrap();
        worker
            .send_request(|reply| ComRequest::ListServers {
                host: "localhost".to_string(),
                reply,
            })
            .await
            .unwrap();
        assert_eq!(state.connect_count.load(Ordering::Relaxed), 1);
    }
}
