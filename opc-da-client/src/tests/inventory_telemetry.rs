//! Portable native operation scope characterization.

use crate::inventory_telemetry::{
    InventoryNativeOperationTelemetryCollector, install_native_operation_telemetry,
    record_native_operation,
};
use crate::provider::InventoryNativeOperationKind;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

#[test]
fn thread_local_recording_recovers_poison_without_discarding_samples() {
    let recorder = Arc::new(Mutex::new(
        InventoryNativeOperationTelemetryCollector::default(),
    ));
    lock_native_operation_telemetry(&recorder).record(
        InventoryNativeOperationKind::Da3Page,
        Duration::from_nanos(1),
    );
    let poisoned = Arc::clone(&recorder);
    assert!(
        std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison only the test telemetry collector");
        })
        .join()
        .is_err()
    );
    let scope = install_native_operation_telemetry(Arc::clone(&recorder));
    record_native_operation(
        InventoryNativeOperationKind::GetItemId,
        Duration::from_nanos(2),
    );
    drop(scope);
    let observations = lock_native_operation_telemetry(&recorder).take();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].kind, InventoryNativeOperationKind::Da3Page);
    assert_eq!(observations[0].total_elapsed, Duration::from_nanos(1));
    assert_eq!(
        observations[1].kind,
        InventoryNativeOperationKind::GetItemId
    );
    assert_eq!(observations[1].total_elapsed, Duration::from_nanos(2));
}

#[test]
fn native_numeric_telemetry_saturates_without_panicking() {
    let mut stats = InventoryNativeOperationStats {
        count: u64::MAX,
        total_elapsed: Duration::MAX,
        max_elapsed: Duration::MAX,
        latency_histogram: InventoryNativeOperationLatencyHistogram::default(),
    };
    stats.record(Duration::from_nanos(1));
    let observation = stats.finish(InventoryNativeOperationKind::Da3Page);
    assert_eq!(observation.count, u64::MAX);
    assert_eq!(observation.total_elapsed, Duration::MAX);
    assert_eq!(observation.max_elapsed, Duration::MAX);
}

#[test]
fn native_operation_telemetry_scopes_are_nested_and_thread_local() {
    let outer = Arc::new(Mutex::new(
        InventoryNativeOperationTelemetryCollector::default(),
    ));
    let inner = Arc::new(Mutex::new(
        InventoryNativeOperationTelemetryCollector::default(),
    ));
    let outer_scope = install_native_operation_telemetry(Arc::clone(&outer));
    record_native_operation(
        InventoryNativeOperationKind::Da3Page,
        Duration::from_nanos(1),
    );
    {
        let _inner_scope = install_native_operation_telemetry(Arc::clone(&inner));
        record_native_operation(
            InventoryNativeOperationKind::GetItemId,
            Duration::from_nanos(2),
        );
        std::thread::spawn(|| {
            record_native_operation(
                InventoryNativeOperationKind::Da3Page,
                Duration::from_nanos(100),
            );
        })
        .join()
        .unwrap();
    }
    record_native_operation(
        InventoryNativeOperationKind::Da3Page,
        Duration::from_nanos(3),
    );
    drop(outer_scope);
    record_native_operation(
        InventoryNativeOperationKind::Da3Page,
        Duration::from_nanos(100),
    );

    let outer = outer.lock().unwrap().take();
    let inner = inner.lock().unwrap().take();
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].kind, InventoryNativeOperationKind::Da3Page);
    assert_eq!(outer[0].count, 2);
    assert_eq!(outer[0].total_elapsed, Duration::from_nanos(4));
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].kind, InventoryNativeOperationKind::GetItemId);
    assert_eq!(inner[0].count, 1);
    assert_eq!(inner[0].total_elapsed, Duration::from_nanos(2));
}
