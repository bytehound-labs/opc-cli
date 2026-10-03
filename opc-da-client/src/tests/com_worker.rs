//! Worker affinity, connection retry, read/write, and lifecycle characterization.

#![allow(
    clippy::single_char_pattern,
    clippy::cast_possible_wrap,
    clippy::ptr_as_ptr,
    clippy::borrow_as_ptr,
    clippy::mixed_attributes_style,
    clippy::unreadable_literal,
    clippy::manual_assert
)]
use crate::bindings::da::{OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_LEAF, OPC_NS_FLAT};
use crate::com_worker::{ComRequest, ComWorker, ReadPresentation};
use crate::errors::{OpcError, OpcResult};
use crate::helpers::opc_value_to_variant;
use crate::opc_da::typedefs::GroupHandle;
use crate::provider::{BrowsePageRequest, OpcValue};
use std::sync::Arc;
use tokio::sync::oneshot;

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
    operation_threads: Mutex<Vec<std::thread::ThreadId>>,
    server_drop_threads: Mutex<Vec<std::thread::ThreadId>>,
}

struct ConfigurableMockConnector {
    state: Arc<MockState>,
}

struct ConfigurableMockServer {
    state: Arc<MockState>,
}

impl Drop for ConfigurableMockServer {
    fn drop(&mut self) {
        self.state
            .server_drop_threads
            .lock()
            .unwrap()
            .push(std::thread::current().id());
    }
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

        // SAFETY: Allocate one suitably aligned OPC item result with the COM
        // allocator; the owning RemoteArray releases it with CoTaskMemFree.
        let res_ptr = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<tagOPCITEMRESULT>())
        } as *mut tagOPCITEMRESULT;
        assert!(!res_ptr.is_null());
        // SAFETY: The non-null allocation has space for one item result and
        // is uniquely owned; initialize it before constructing RemoteArray.
        unsafe {
            std::ptr::write(res_ptr, res);
        }
        let res_array = RemoteArray::from_mut_ptr(res_ptr, 1);

        // SAFETY: Allocate one HRESULT with the COM allocator for RemoteArray.
        let err_ptr = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(
                std::mem::size_of::<windows::core::HRESULT>(),
            )
        } as *mut windows::core::HRESULT;
        assert!(!err_ptr.is_null());
        // SAFETY: The checked allocation holds one aligned HRESULT; no other
        // pointer aliases this write before ownership passes to RemoteArray.
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
        // SAFETY: Allocate one aligned OPC item state with the COM allocator;
        // RemoteArray takes ownership after initialization.
        let state_ptr = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(std::mem::size_of::<tagOPCITEMSTATE>())
        } as *mut tagOPCITEMSTATE;
        assert!(!state_ptr.is_null());
        // SAFETY: The checked allocation fits one item state. This write moves
        // the state and its VARIANT into the uniquely owned allocation.
        unsafe {
            std::ptr::write(state_ptr, item_state);
        }

        // SAFETY: Allocate one HRESULT with the COM allocator for RemoteArray.
        let error_ptr = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(
                std::mem::size_of::<windows::core::HRESULT>(),
            )
        } as *mut windows::core::HRESULT;
        assert!(!error_ptr.is_null());
        // SAFETY: The non-null allocation is aligned, sized for one HRESULT,
        // and remains uniquely owned until RemoteArray is constructed.
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

        // SAFETY: Allocate one HRESULT with the COM allocator for RemoteArray.
        let hr_ptr = unsafe {
            windows::Win32::System::Com::CoTaskMemAlloc(
                std::mem::size_of::<windows::core::HRESULT>(),
            )
        } as *mut windows::core::HRESULT;
        assert!(!hr_ptr.is_null());
        // SAFETY: The checked allocation holds one HRESULT and is uniquely
        // owned; initialize it before transferring ownership to RemoteArray.
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
        self.state
            .operation_threads
            .lock()
            .unwrap()
            .push(std::thread::current().id());
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
            self.state
                .operation_threads
                .lock()
                .unwrap()
                .push(std::thread::current().id());
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

#[test]
fn com_worker_events_keep_the_public_module_target() {
    crate::tests::tracing::assert_event_targets("opc_da_client::com_worker", || {
        drop(ComWorker::<WorkerMockConnector>::closed());
    });
}

#[tokio::test]
async fn cached_server_operations_and_destruction_stay_on_the_worker_thread() {
    let caller_thread = std::thread::current().id();
    let state = Arc::new(MockState::default());
    let connector = Arc::new(ConfigurableMockConnector {
        state: Arc::clone(&state),
    });
    let mut worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
        .await
        .unwrap();
    let worker_thread = worker.handle.as_ref().unwrap().thread().id();
    assert_ne!(caller_thread, worker_thread);

    worker
        .send_request(|reply| ComRequest::WriteTagValue {
            server: "Mock.Server".to_string(),
            tag_id: "Tag".to_string(),
            value: OpcValue::Int(1),
            reply,
        })
        .await
        .unwrap();
    assert_eq!(
        *state.operation_threads.lock().unwrap(),
        vec![worker_thread, worker_thread]
    );
    assert_eq!(
        *state.server_drop_threads.lock().unwrap(),
        Vec::<std::thread::ThreadId>::new()
    );

    let handle = worker.handle.take().unwrap();
    drop(worker);
    tokio::task::spawn_blocking(move || handle.join().unwrap())
        .await
        .unwrap();
    assert_eq!(
        *state.server_drop_threads.lock().unwrap(),
        vec![worker_thread]
    );
}

#[tokio::test]
async fn test_worker_starts_and_stops() {
    let worker =
        tokio::task::spawn_blocking(|| ComWorker::start(Arc::new(WorkerMockConnector)).unwrap())
            .await
            .unwrap();
    drop(worker);
}

#[tokio::test]
async fn test_worker_list_servers() {
    let worker =
        tokio::task::spawn_blocking(|| ComWorker::start(Arc::new(WorkerMockConnector)).unwrap())
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
    let worker =
        tokio::task::spawn_blocking(|| ComWorker::start(Arc::new(MismatchedConnector)).unwrap())
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
            msg.contains("shut down") || msg.contains("channel closed") || msg.contains("panicked"),
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

    let worker =
        tokio::task::spawn_blocking(|| ComWorker::start(Arc::new(FailingInitConnector)).unwrap())
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

#[test]
fn public_com_worker_paths_remain_available() {
    let worker = crate::com_worker::ComWorker::<WorkerMockConnector>::closed();
    assert!(worker.handle.is_none());

    let (reply, _receiver) = oneshot::channel();
    let request = crate::com_worker::ComRequest::ReadTagValues {
        server: "Mock.Server".to_string(),
        tag_ids: vec!["Tag".to_string()],
        presentation: crate::com_worker::ReadPresentation::Display,
        reply,
    };
    assert!(matches!(
        request,
        crate::com_worker::ComRequest::ReadTagValues {
            presentation: crate::com_worker::ReadPresentation::Display,
            ..
        }
    ));
}
