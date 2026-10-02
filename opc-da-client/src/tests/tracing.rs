use std::sync::{Arc, Mutex};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

struct TargetSubscriber(Arc<Mutex<Vec<&'static str>>>);

impl Subscriber for TargetSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        self.0.lock().unwrap().push(event.metadata().target());
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

pub fn assert_event_targets(expected: &str, operation: impl FnOnce()) {
    let targets = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(TargetSubscriber(Arc::clone(&targets)), operation);
    let targets = targets.lock().unwrap().clone();
    assert_ne!(
        targets,
        Vec::<&str>::new(),
        "the operation must emit an event"
    );
    assert!(
        targets.iter().all(|target| *target == expected),
        "unexpected tracing targets: {targets:?}"
    );
}
