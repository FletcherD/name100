//! Miss logging. Alone in its own test binary because it installs a
//! process-wide `tracing` subscriber.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Index, MatchConfig, Matcher};
use hundred_server::{App, MISS_TARGET};
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

#[derive(Default)]
struct Captured {
    reason: Option<String>,
    normalized: Option<String>,
}

impl Visit for Captured {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "reason" {
            self.reason = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "normalized" {
            self.normalized = Some(format!("{value:?}"));
        }
    }
}

/// Collects (reason, normalized) from every miss event.
#[derive(Clone, Default)]
struct MissLog(Arc<Mutex<Vec<(String, String)>>>);

impl<S: tracing::Subscriber> Layer<S> for MissLog {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != MISS_TARGET {
            return;
        }
        let mut captured = Captured::default();
        event.record(&mut captured);
        self.0.lock().unwrap().push((captured.reason.unwrap(), captured.normalized.unwrap()));
    }
}

#[tokio::test]
async fn not_found_and_ambiguous_verdicts_log_a_miss_event() {
    let log = MissLog::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(log.clone()))
        .unwrap();

    let dump = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.ndjson");
    let dir = tempfile::tempdir().unwrap();
    build_index(open_dump(dump.to_str().unwrap()).unwrap(), dir.path(), &BuildOptions::default())
        .unwrap();
    let app = App::new(Matcher::new(Index::open(dir.path()).unwrap(), MatchConfig::default()));

    for query in ["Oh+fuck+off", "Cher", "Elon+Musk", "JENNIFER", "1234", "Hermione+Granger"] {
        let request =
            Request::builder().uri(format!("/v1/lookup?q={query}")).body(Body::empty()).unwrap();
        assert_eq!(app.router().oneshot(request).await.unwrap().status(), StatusCode::OK);
    }
    let events = log.0.lock().unwrap().clone();
    assert_eq!(
        events,
        [
            ("not_found".to_owned(), "oh fuck off".to_owned()),
            ("ambiguous".to_owned(), "jennifer".to_owned()),
        ]
    );
}
