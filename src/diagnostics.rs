//! Process-local selection diagnostics containing only fixed labels and numbers.
//!
//! No API accepts text, window titles, paths, credentials, or other application
//! data. Nothing is written to disk or sent over the network. Hook callbacks
//! should use only [`increment`], which performs one relaxed atomic operation;
//! decision recording and report formatting belong outside those callbacks.

use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard, OnceLock,
    },
    time::Instant,
};

use serde::Serialize;

/// Maximum number of recent decisions retained in memory.
pub const TRACE_CAPACITY: usize = 32;

/// Fixed counters, including those safe to update in a low-level mouse callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[repr(usize)]
pub enum Counter {
    HookMove,
    HookDown,
    HookUp,
    ExternalDown,
    NotifyPosted,
    NotifyFailed,
    AppDownAccepted,
    AppDownRejected,
    AppUpAccepted,
    AppUpRejected,
    CaptureQueued,
    CaptureSuccess,
    CaptureEmpty,
    CaptureFailed,
    RenderAttempt,
    RenderSuccess,
    RenderFailed,
    HookInstallAttempt,
    HookInstallSuccess,
    HookRebind,
    UiEventDropped,
    AutoProbeTimer,
}

impl Counter {
    const ALL: [Self; 22] = [
        Self::HookMove,
        Self::HookDown,
        Self::HookUp,
        Self::ExternalDown,
        Self::NotifyPosted,
        Self::NotifyFailed,
        Self::AppDownAccepted,
        Self::AppDownRejected,
        Self::AppUpAccepted,
        Self::AppUpRejected,
        Self::CaptureQueued,
        Self::CaptureSuccess,
        Self::CaptureEmpty,
        Self::CaptureFailed,
        Self::RenderAttempt,
        Self::RenderSuccess,
        Self::RenderFailed,
        Self::HookInstallAttempt,
        Self::HookInstallSuccess,
        Self::HookRebind,
        Self::UiEventDropped,
        Self::AutoProbeTimer,
    ];
}

/// The selection pipeline stage making a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Stage {
    HookInstall,
    HookHealth,
    HookNotification,
    SelectionEventsInstall,
    AppDown,
    AppUp,
    CaptureQueue,
    Capture,
    Render,
    SelfTest,
}

/// Fixed decisions; error messages and captured text must never be recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Started,
    Succeeded,
    Failed,
    Accepted,
    Disabled,
    Paused,
    ManualMode,
    OwnWindow,
    Stale,
    Busy,
    Empty,
    Protected,
    NoSelection,
    TooLong,
    Duplicate,
    Retrying,
    Ineligible,
    Cancelled,
    TimedOut,
    Unavailable,
    Skipped,
    PopupHidden,
    ForegroundChanged,
    WorkerStopped,
    NotifyDropped,
    UnknownError,
}

/// A recent decision. Time is relative to the first recorded decision, not a
/// wall-clock timestamp, and the optional code is a numeric system error only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub sequence: u64,
    pub elapsed_ms: u64,
    pub stage: Stage,
    pub outcome: Outcome,
    pub error_code: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CounterValue {
    pub counter: Counter,
    pub value: u64,
}

/// A bounded copy suitable for explicitly showing or copying in the UI.
/// Counters are sampled independently while other threads may still update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    version: &'static str,
    pub counters: Vec<CounterValue>,
    /// Oldest retained decision first.
    pub decisions: Vec<Decision>,
}

impl Snapshot {
    /// Produce a human-readable report using fixed field names and values only.
    pub fn report(&self) -> String {
        let mut report = format!(
            "LingoLeaf {} selection diagnostics\r\n\
             Memory only; counters and fixed decisions. No selected text, window titles, or credentials.\r\n\
             Counters (since app start):\r\n",
            self.version
        );
        for counter in &self.counters {
            let _ = writeln!(report, "{:?}: {}\r", counter.counter, counter.value);
        }
        let _ = writeln!(
            report,
            "Recent decisions (up to {TRACE_CAPACITY}, oldest first):\r"
        );
        if self.decisions.is_empty() {
            report.push_str("None\r\n");
        }
        for decision in &self.decisions {
            let _ = write!(
                report,
                "#{} +{}ms {:?}: {:?}",
                decision.sequence, decision.elapsed_ms, decision.stage, decision.outcome
            );
            if let Some(code) = decision.error_code {
                let _ = write!(report, " (system error {code})");
            }
            report.push_str("\r\n");
        }
        report
    }
}

struct Trace {
    entries: [Option<Decision>; TRACE_CAPACITY],
    next: usize,
    len: usize,
    sequence: u64,
}

impl Trace {
    const fn new() -> Self {
        Self {
            entries: [None; TRACE_CAPACITY],
            next: 0,
            len: 0,
            sequence: 0,
        }
    }

    fn push(&mut self, elapsed_ms: u64, stage: Stage, outcome: Outcome, error_code: Option<u32>) {
        self.sequence = self.sequence.saturating_add(1);
        self.entries[self.next] = Some(Decision {
            sequence: self.sequence,
            elapsed_ms,
            stage,
            outcome,
            error_code,
        });
        self.next = (self.next + 1) % TRACE_CAPACITY;
        self.len = (self.len + 1).min(TRACE_CAPACITY);
    }

    fn snapshot(&self) -> Vec<Decision> {
        let start = (self.next + TRACE_CAPACITY - self.len) % TRACE_CAPACITY;
        (0..self.len)
            .filter_map(|offset| self.entries[(start + offset) % TRACE_CAPACITY])
            .collect()
    }
}

struct Recorder {
    counters: [AtomicU64; Counter::ALL.len()],
    started: OnceLock<Instant>,
    trace: Mutex<Trace>,
}

impl Recorder {
    const fn new() -> Self {
        Self {
            counters: [const { AtomicU64::new(0) }; Counter::ALL.len()],
            started: OnceLock::new(),
            trace: Mutex::new(Trace::new()),
        }
    }

    fn increment(&self, counter: Counter) {
        self.counters[counter as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn trace(&self) -> MutexGuard<'_, Trace> {
        // Diagnostics must remain available even if a recording thread panics.
        self.trace.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn record(&self, stage: Stage, outcome: Outcome, error_code: Option<u32>) {
        let elapsed = self.started.get_or_init(Instant::now).elapsed().as_millis();
        let elapsed_ms = elapsed.min(u128::from(u64::MAX)) as u64;
        self.trace().push(elapsed_ms, stage, outcome, error_code);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: env!("CARGO_PKG_VERSION"),
            counters: Counter::ALL
                .iter()
                .map(|&counter| CounterValue {
                    counter,
                    value: self.counters[counter as usize].load(Ordering::Relaxed),
                })
                .collect(),
            decisions: self.trace().snapshot(),
        }
    }
}

static RECORDER: Recorder = Recorder::new();

/// Increment a fixed counter without locking or allocating.
pub fn increment(counter: Counter) {
    RECORDER.increment(counter);
}

/// Record a fixed decision outside low-level callbacks.
pub fn record(stage: Stage, outcome: Outcome) {
    RECORDER.record(stage, outcome, None);
}

/// Record a numeric system error, never a provider's message or arbitrary text.
pub fn record_error(stage: Stage, outcome: Outcome, error_code: u32) {
    RECORDER.record(stage, outcome, Some(error_code));
}

/// Copy the current counters and bounded trace. This does not persist or share it.
pub fn snapshot() -> Snapshot {
    RECORDER.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_retains_only_the_latest_decisions_in_order() {
        let recorder = Recorder::new();
        for code in 0..100 {
            recorder.record(Stage::Capture, Outcome::Failed, Some(code));
        }
        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.decisions.len(), TRACE_CAPACITY);
        assert_eq!(snapshot.decisions.first().unwrap().sequence, 69);
        assert_eq!(snapshot.decisions.first().unwrap().error_code, Some(68));
        assert_eq!(snapshot.decisions.last().unwrap().sequence, 100);
        assert_eq!(snapshot.decisions.last().unwrap().error_code, Some(99));
    }

    #[test]
    fn concurrent_updates_preserve_counts_without_filling_the_trace() {
        let recorder = Recorder::new();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let recorder = &recorder;
                scope.spawn(move || {
                    for _ in 0..1000 {
                        recorder.increment(Counter::HookMove);
                    }
                });
            }
        });
        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.counters[Counter::HookMove as usize].value, 4000);
        assert!(snapshot.decisions.is_empty());
    }

    #[test]
    fn report_and_serialization_contain_only_fixed_labels_and_numeric_metadata() {
        let recorder = Recorder::new();
        recorder.increment(Counter::CaptureFailed);
        recorder.record(Stage::Capture, Outcome::Protected, Some(5));
        let snapshot = recorder.snapshot();
        let report = snapshot.report();
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(report.contains("CaptureFailed: 1"));
        assert!(report.contains("Capture: Protected (system error 5)"));
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["decisions"][0]["stage"], "Capture");
        assert_eq!(json["decisions"][0]["outcome"], "Protected");
        assert_eq!(json["decisions"][0]["error_code"], 5);
        let mut keys: Vec<_> = json["decisions"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["elapsed_ms", "error_code", "outcome", "sequence", "stage"]
        );
    }

    #[test]
    fn poisoned_trace_still_accepts_and_reports_decisions() {
        let recorder = Recorder::new();
        let _ = std::panic::catch_unwind(|| {
            let _guard = recorder.trace.lock().unwrap();
            panic!("simulate interrupted recording");
        });
        recorder.record(Stage::HookHealth, Outcome::Retrying, None);
        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.decisions.len(), 1);
        assert_eq!(snapshot.decisions[0].outcome, Outcome::Retrying);
    }
}
