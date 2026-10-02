//! Portable pacing and typed operation characterization.

use crate::errors::OpcError;
use crate::inventory_boundary::{InventoryBoundary, paced_call};
use crate::provider::{InventoryControl, InventoryNativeOperationKind};

use super::*;

#[test]
fn boundary_events_keep_the_inventory_target() {
    crate::tests::tracing::assert_event_targets("opc_da_client::inventory", || {
        let control = InventoryControl::new();
        let mut boundary = InventoryBoundary::new(&control);
        assert_eq!(
            boundary.before_operation_with_cost(1),
            BoundaryResult::Proceed
        );
    });
}

#[test]
fn cancellation_does_not_enter_or_count_the_operation() {
    let control = InventoryControl::new();
    let mut boundary = InventoryBoundary::new(&control);
    assert!(std::ptr::eq(boundary.control(), &control));
    boundary.control().cancel();
    let result: Result<(), _> = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da3Page,
        1,
        || panic!("a cancelled boundary must not enter the native operation"),
    );
    assert!(matches!(result, Err(InventoryError::Cancelled)));
    assert_eq!(boundary.operations(), 0);
    assert_eq!(boundary.paused_time(), Duration::ZERO);
    assert_eq!(
        boundary.take_operation_observations(),
        Vec::<InventoryNativeOperationObservation>::new()
    );
}

#[test]
fn entered_failures_are_counted_and_scoped_calls_keep_their_kind() {
    use crate::inventory_telemetry::{install_native_operation_telemetry, record_native_operation};

    let control = InventoryControl::new();
    let mut boundary = InventoryBoundary::new(&control);
    let _scope = install_native_operation_telemetry(boundary.telemetry_recorder());
    let result = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da3Page,
        100,
        || {
            record_native_operation(
                InventoryNativeOperationKind::GetItemId,
                Duration::from_nanos(3),
            );
            Err::<(), _>(OpcError::Internal("synthetic".to_string()))
        },
    );
    assert!(matches!(
        result,
        Err(InventoryError::Failed(OpcError::Internal(_)))
    ));
    assert_eq!(boundary.operations(), 1);
    let observations = boundary.take_operation_observations();
    assert_eq!(observations.len(), 2);
    assert_eq!(
        observations[0].kind,
        InventoryNativeOperationKind::GetItemId
    );
    assert_eq!(observations[0].total_elapsed, Duration::from_nanos(3));
    assert_eq!(observations[1].kind, InventoryNativeOperationKind::Da3Page);
    assert_eq!(observations[1].count, 1);
}

#[test]
fn boundary_record_and_take_recover_poisoned_numeric_telemetry() {
    let control = InventoryControl::new();
    let boundary = InventoryBoundary::new(&control);
    let recorder = boundary.telemetry_recorder();
    assert!(
        std::thread::spawn(move || {
            let _guard = recorder.lock().unwrap();
            panic!("poison only the test telemetry collector");
        })
        .join()
        .is_err()
    );
    boundary.record_operation(
        InventoryNativeOperationKind::Da3Page,
        Duration::from_nanos(2),
    );
    let observations = boundary.take_operation_observations();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].count, 1);
    assert_eq!(observations[0].total_elapsed, Duration::from_nanos(2));
}

#[test]
#[allow(clippy::too_many_lines)]
fn paced_calls_record_typed_native_operation_observations_in_order() {
    let control = InventoryControl::new();
    let mut boundary = InventoryBoundary::new(&control);

    let result_a = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::NamespaceOrganizationQuery,
        1,
        || Ok::<_, OpcError>("namespace"),
    )
    .unwrap();
    let result_b = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2BranchEnumeratorCreation,
        2,
        || Ok::<_, OpcError>("branch"),
    )
    .unwrap();
    let result_c = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2LeafEnumeratorCreation,
        3,
        || Ok::<_, OpcError>("leaf"),
    )
    .unwrap();
    let result_d = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2FlatEnumeratorCreation,
        4,
        || Ok::<_, OpcError>("flat"),
    )
    .unwrap();
    let result_e = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2StringRefill,
        256,
        || Ok::<_, OpcError>("refill"),
    )
    .unwrap();
    let result_f = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::GetItemId,
        1,
        || Ok::<_, OpcError>("item"),
    )
    .unwrap();
    let result_g = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2ClassificationDown,
        1,
        || Ok::<_, OpcError>("classify-down"),
    )
    .unwrap();
    let result_h = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2ClassificationUp,
        1,
        || Ok::<_, OpcError>("classify-up"),
    )
    .unwrap();
    let result_i = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2PathDown,
        1,
        || Ok::<_, OpcError>("path-down"),
    )
    .unwrap();
    let result_j = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2PathUp,
        1,
        || Ok::<_, OpcError>("path-up"),
    )
    .unwrap();
    let result_k = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2ProbeDown,
        1,
        || Ok::<_, OpcError>("probe-down"),
    )
    .unwrap();
    let result_l = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da2ProbeUp,
        1,
        || Ok::<_, OpcError>("probe-up"),
    )
    .unwrap();
    let result_m = paced_call(
        &mut boundary,
        InventoryNativeOperationKind::Da3Page,
        10,
        || Ok::<_, OpcError>("page"),
    )
    .unwrap();

    assert_eq!(result_a, "namespace");
    assert_eq!(result_b, "branch");
    assert_eq!(result_c, "leaf");
    assert_eq!(result_d, "flat");
    assert_eq!(result_e, "refill");
    assert_eq!(result_f, "item");
    assert_eq!(result_g, "classify-down");
    assert_eq!(result_h, "classify-up");
    assert_eq!(result_i, "path-down");
    assert_eq!(result_j, "path-up");
    assert_eq!(result_k, "probe-down");
    assert_eq!(result_l, "probe-up");
    assert_eq!(result_m, "page");

    let observations = boundary.take_operation_observations();
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![
            InventoryNativeOperationKind::NamespaceOrganizationQuery,
            InventoryNativeOperationKind::Da2BranchEnumeratorCreation,
            InventoryNativeOperationKind::Da2LeafEnumeratorCreation,
            InventoryNativeOperationKind::Da2FlatEnumeratorCreation,
            InventoryNativeOperationKind::Da2StringRefill,
            InventoryNativeOperationKind::GetItemId,
            InventoryNativeOperationKind::Da2ClassificationDown,
            InventoryNativeOperationKind::Da2ClassificationUp,
            InventoryNativeOperationKind::Da2PathDown,
            InventoryNativeOperationKind::Da2PathUp,
            InventoryNativeOperationKind::Da2ProbeDown,
            InventoryNativeOperationKind::Da2ProbeUp,
            InventoryNativeOperationKind::Da3Page,
        ]
    );
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.count)
            .collect::<Vec<_>>(),
        vec![1; 13]
    );
    assert!(observations.iter().all(|observation| {
        observation
            .latency_histogram
            .bucket_counts
            .iter()
            .sum::<u64>()
            == observation.count
    }));
    assert!(
        observations
            .iter()
            .all(|observation| observation.total_elapsed >= observation.max_elapsed)
    );
}
