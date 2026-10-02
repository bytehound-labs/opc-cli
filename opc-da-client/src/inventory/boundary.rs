//! Pause, cancellation, and native-start pacing boundaries.

use crate::errors::OpcResult;
use crate::inventory_error::InventoryError;
use crate::inventory_telemetry::{
    InventoryNativeOperationTelemetryCollector, lock_native_operation_telemetry,
};
use crate::provider::{
    InventoryControl, InventoryNativeOperationKind, InventoryNativeOperationObservation,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryResult {
    Proceed,
    Cancelled,
}

/// Gate every bounded native operation on pause/cancellation and current pacing.
pub struct InventoryBoundary<'a> {
    control: &'a InventoryControl,
    last_started: Option<Instant>,
    paused_time: Duration,
    native_operations: u64,
    first_operation_reported: bool,
    telemetry: Arc<Mutex<InventoryNativeOperationTelemetryCollector>>,
}

pub fn pacing_interval(pacing: crate::provider::InventoryPacing, item_cost: u32) -> Duration {
    let item_interval = pacing
        .item_rate_per_second
        .filter(|rate| *rate > 0)
        .map_or(Duration::ZERO, |rate| {
            Duration::from_secs_f64(f64::from(item_cost.max(1)) / f64::from(rate))
        });
    pacing.min_interval.max(item_interval)
}

impl<'a> InventoryBoundary<'a> {
    pub fn control(&self) -> &'a InventoryControl {
        self.control
    }

    pub fn new(control: &'a InventoryControl) -> Self {
        Self {
            control,
            last_started: None,
            paused_time: Duration::ZERO,
            native_operations: 0,
            first_operation_reported: false,
            telemetry: Arc::new(Mutex::new(
                InventoryNativeOperationTelemetryCollector::default(),
            )),
        }
    }

    pub fn before_operation_with_cost(&mut self, item_cost: u32) -> BoundaryResult {
        loop {
            if self.control.is_cancelled() {
                return BoundaryResult::Cancelled;
            }

            if self.control.is_paused() {
                let started = Instant::now();
                while self.control.is_paused() && !self.control.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.paused_time += started.elapsed();
                continue;
            }

            let interval = pacing_interval(self.control.pacing(), item_cost);
            if let Some(last_started) = self.last_started {
                let elapsed = last_started.elapsed();
                if let Some(remaining) = interval.checked_sub(elapsed) {
                    std::thread::sleep(remaining.min(Duration::from_millis(10)));
                    continue;
                }
            }

            if self.control.is_cancelled() || self.control.is_paused() {
                continue;
            }
            if !self.first_operation_reported {
                let pacing = self.control.pacing();
                tracing::info!(target: "opc_da_client::inventory",
                    item_cost,
                    item_rate_per_second = ?pacing.item_rate_per_second,
                    min_interval_ms = pacing.min_interval.as_millis(),
                    "native inventory first operation starting"
                );
                self.first_operation_reported = true;
            }
            self.last_started = Some(Instant::now());
            self.native_operations = self.native_operations.saturating_add(1);
            return BoundaryResult::Proceed;
        }
    }

    pub fn operations(&self) -> u64 {
        self.native_operations
    }

    pub fn paused_time(&self) -> Duration {
        self.paused_time
    }

    pub fn record_operation(&self, kind: InventoryNativeOperationKind, elapsed: Duration) {
        lock_native_operation_telemetry(&self.telemetry).record(kind, elapsed);
    }

    pub fn take_operation_observations(&self) -> Vec<InventoryNativeOperationObservation> {
        lock_native_operation_telemetry(&self.telemetry).take()
    }

    pub fn telemetry_recorder(&self) -> Arc<Mutex<InventoryNativeOperationTelemetryCollector>> {
        Arc::clone(&self.telemetry)
    }
}

pub fn paced_call<T>(
    boundary: &mut InventoryBoundary<'_>,
    kind: InventoryNativeOperationKind,
    item_cost: u32,
    operation: impl FnOnce() -> OpcResult<T>,
) -> Result<T, InventoryError> {
    match boundary.before_operation_with_cost(item_cost) {
        BoundaryResult::Proceed => {
            let started = Instant::now();
            let result = operation().map_err(InventoryError::from);
            boundary.record_operation(kind, started.elapsed());
            result
        }
        BoundaryResult::Cancelled => Err(InventoryError::Cancelled),
    }
}

#[cfg(test)]
#[path = "../tests/inventory_boundary.rs"]
mod tests;
