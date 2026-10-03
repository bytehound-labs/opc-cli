//! Thread-local, first-seen native operation accounting.

use crate::provider::{
    InventoryNativeOperationKind, InventoryNativeOperationLatencyHistogram,
    InventoryNativeOperationObservation,
};
use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

#[derive(Debug, Default)]
struct InventoryNativeOperationStats {
    count: u64,
    total_elapsed: Duration,
    max_elapsed: Duration,
    latency_histogram: InventoryNativeOperationLatencyHistogram,
}

impl InventoryNativeOperationStats {
    fn record(&mut self, elapsed: Duration) {
        self.count = self.count.saturating_add(1);
        self.total_elapsed = self.total_elapsed.saturating_add(elapsed);
        self.max_elapsed = self.max_elapsed.max(elapsed);
        self.latency_histogram.record(elapsed);
    }

    fn finish(self, kind: InventoryNativeOperationKind) -> InventoryNativeOperationObservation {
        let percentiles = self
            .latency_histogram
            .percentiles(self.count, self.max_elapsed);
        InventoryNativeOperationObservation {
            kind,
            count: self.count,
            total_elapsed: self.total_elapsed,
            max_elapsed: self.max_elapsed,
            latency_histogram: self.latency_histogram,
            percentiles,
        }
    }
}

#[derive(Debug, Default)]
pub struct InventoryNativeOperationTelemetryCollector {
    observations: Vec<(InventoryNativeOperationKind, InventoryNativeOperationStats)>,
}

impl InventoryNativeOperationTelemetryCollector {
    pub fn record(&mut self, kind: InventoryNativeOperationKind, elapsed: Duration) {
        if let Some((_, stats)) = self
            .observations
            .iter_mut()
            .find(|(existing_kind, _)| *existing_kind == kind)
        {
            stats.record(elapsed);
            return;
        }

        let mut stats = InventoryNativeOperationStats::default();
        stats.record(elapsed);
        self.observations.push((kind, stats));
    }

    pub fn take(&mut self) -> Vec<InventoryNativeOperationObservation> {
        self.observations
            .drain(..)
            .map(|(kind, stats)| stats.finish(kind))
            .collect()
    }
}

pub fn lock_native_operation_telemetry(
    collector: &Mutex<InventoryNativeOperationTelemetryCollector>,
) -> MutexGuard<'_, InventoryNativeOperationTelemetryCollector> {
    // Only best-effort numeric telemetry lives here, not native resources.
    // Poison recovery must not turn an accounting failure into inventory failure.
    collector.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    static CURRENT_NATIVE_OPERATION_TELEMETRY: RefCell<
        Option<Arc<Mutex<InventoryNativeOperationTelemetryCollector>>>,
    > = const { RefCell::new(None) };
}

pub struct NativeOperationTelemetryScope {
    previous: Option<Arc<Mutex<InventoryNativeOperationTelemetryCollector>>>,
}

impl Drop for NativeOperationTelemetryScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_NATIVE_OPERATION_TELEMETRY.with(|slot| {
            let _ = slot.replace(previous);
        });
    }
}

pub fn install_native_operation_telemetry(
    recorder: Arc<Mutex<InventoryNativeOperationTelemetryCollector>>,
) -> NativeOperationTelemetryScope {
    let previous = CURRENT_NATIVE_OPERATION_TELEMETRY.with(|slot| slot.replace(Some(recorder)));
    NativeOperationTelemetryScope { previous }
}

pub fn record_native_operation(kind: InventoryNativeOperationKind, elapsed: Duration) {
    CURRENT_NATIVE_OPERATION_TELEMETRY.with(|slot| {
        let Some(recorder) = slot.borrow().as_ref().cloned() else {
            return;
        };
        lock_native_operation_telemetry(&recorder).record(kind, elapsed);
    });
}

#[cfg(test)]
#[path = "../tests/inventory_telemetry.rs"]
mod tests;
