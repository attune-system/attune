use super::*;
use futures::FutureExt;
use std::io::{self, Write};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture(run: impl FnOnce()) -> Vec<serde_json::Value> {
    let output = Capture::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_ansi(false)
        .json()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = output.0.lock().unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn fields(events: &[serde_json::Value], event: &str) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|value| value["fields"]["event"] == event)
        .map(|value| value["fields"].clone())
        .collect()
}

#[test]
fn reentry_preserves_first_request_and_acquisition_and_one_holder() {
    let events = capture(|| {
        let trace = SharedGraphTransactionTrace::new("test.shared_owner");
        trace.begun();
        trace
            .observe(async {
                shared_graph_admission_requested().finished(Some(42));
                let original = {
                    let state = lock(&trace.state);
                    (state.first_requested, state.first_acquired)
                };
                trace.stage("nested_authorization");
                shared_graph_admission_requested().finished(Some(42));
                let state = lock(&trace.state);
                assert_eq!(original, (state.first_requested, state.first_acquired));
            })
            .now_or_never()
            .expect("observer must not add a suspension");
        // Finishing the observed future restores the previous task-local scope.
        shared_graph_admission_requested().finished(Some(42));
        trace.stage("commit");
        trace.committed();
        trace.operation_result("success");
    });
    assert_eq!(fields(&events, "admission_requested").len(), 1);
    assert_eq!(fields(&events, "admission_acquired").len(), 1);
    assert_eq!(fields(&events, "admission_reentered").len(), 1);
    let completed = fields(&events, "transaction_completed");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["completion"], "committed");
    assert_eq!(completed[0]["backend"], 42);
    assert_eq!(
        completed[0]["hold_us"],
        fields(&events, "operation_result")[0]["hold_us"]
    );
    assert!(fields(&events, "owner_dropped").is_empty());
    let id = &completed[0]["transaction_id"];
    assert!(events
        .iter()
        .all(|event| event["fields"]["transaction_id"] == *id));
}

#[test]
fn acknowledged_rollback_is_not_reported_as_unknown() {
    let events = capture(|| {
        let trace = SharedGraphTransactionTrace::new("test.validation_owner");
        trace.begun();
        trace.stage("validation_rollback");
        trace.rolled_back();
        trace.operation_result("rejected");
    });
    assert_eq!(
        fields(&events, "transaction_completed")[0]["completion"],
        "rolled_back"
    );
    assert!(fields(&events, "owner_dropped").is_empty());
}

#[test]
fn error_and_cancelled_owner_never_claim_database_completion() {
    for result in [Some("rejected"), Some("error"), None] {
        let events = capture(|| {
            let future = async {
                let trace = SharedGraphTransactionTrace::new("test.incomplete_owner");
                trace.begun();
                trace
                    .observe(async {
                        shared_graph_admission_requested().finished(Some(42));
                        trace.stage("storage_verification");
                        if let Some(result) = result {
                            trace.operation_result(result);
                        }
                        std::future::pending::<()>().await;
                    })
                    .await;
            };
            // Poll once, then cancel by dropping. No sleeps or global subscriber.
            assert!(future.now_or_never().is_none());
        });
        assert!(fields(&events, "transaction_completed").is_empty());
        let dropped = fields(&events, "owner_dropped");
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0]["completion"], "unknown");
        assert_eq!(dropped[0]["stage"], "storage_verification");
        assert_eq!(dropped[0]["hold_us"], serde_json::Value::Null);
        assert_eq!(
            fields(&events, "operation_result").len(),
            usize::from(result.is_some())
        );
    }
}

#[test]
fn failed_admission_does_not_claim_a_holder() {
    let events = capture(|| {
        let trace = SharedGraphTransactionTrace::new("test.failed_admission_owner");
        trace.begun();
        trace
            .observe(async { shared_graph_admission_requested().finished(None) })
            .now_or_never()
            .expect("observer must not add a suspension");
        trace.operation_result("error");
    });
    assert_eq!(fields(&events, "admission_failed").len(), 1);
    assert!(fields(&events, "admission_acquired").is_empty());
    assert!(fields(&events, "transaction_completed").is_empty());
    assert_eq!(fields(&events, "owner_dropped")[0]["completion"], "unknown");
}

#[test]
fn cancellation_waiting_for_first_admission_has_no_confirmed_holder() {
    let events = capture(|| {
        let trace = SharedGraphTransactionTrace::new("test.waiting_owner");
        trace.begun();
        SHARED_GRAPH_TRANSACTION.sync_scope(trace.state.clone(), || {
            let _request = shared_graph_admission_requested();
        });
    });
    assert_eq!(fields(&events, "admission_requested").len(), 1);
    assert!(fields(&events, "admission_acquired").is_empty());
    assert!(fields(&events, "transaction_completed").is_empty());
    assert_eq!(fields(&events, "owner_dropped")[0]["completion"], "unknown");
}
