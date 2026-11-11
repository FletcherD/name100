//! The lookup server: an axum app over one in-memory index.
//!
//! - `GET /v1/lookup?q=<name>`: the verdict as JSON. Always 200 for a
//!   verdict, including rejections; 400 when `q` is missing.
//! - `GET /healthz`: the loaded index's manifest.
//! - `GET /metrics`: Prometheus text.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response as HttpResponse};
use axum::routing::get;
use axum::{Json, Router};
use hundred_core::{Matcher, Reason, Response, StageTimings};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, PrometheusRecorder};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;

/// `tracing` target of the structured miss events. Every `not_found` and
/// `ambiguous` verdict emits one, with `reason` and `normalized` fields;
/// `scripts/rank_misses.py` ranks them by frequency.
pub const MISS_TARGET: &str = "hundred::miss";

/// Lookups, labelled by `verdict`, `reason` and `matched_via`.
pub const LOOKUPS_TOTAL: &str = "hundred_lookups_total";
/// In-process time of a whole lookup, in seconds.
pub const LOOKUP_SECONDS: &str = "hundred_lookup_seconds";
/// In-process time of one pipeline stage, in seconds, labelled by `stage`.
pub const STAGE_SECONDS: &str = "hundred_lookup_stage_seconds";

/// Histogram bucket bounds, in seconds: 1 µs to 10 ms, since lookups are
/// expected to take microseconds.
const LATENCY_BUCKETS: &[f64] = &[
    1e-6, 2.5e-6, 5e-6, 1e-5, 2.5e-5, 5e-5, 1e-4, 2.5e-4, 5e-4, 1e-3, 2.5e-3, 5e-3, 1e-2,
];

struct AppState {
    matcher: Matcher,
    /// Metrics are recorded into this recorder rather than a process-wide
    /// one, so several apps (as in tests) never share counters.
    recorder: PrometheusRecorder,
    metrics: PrometheusHandle,
}

/// The server's routes over an already-loaded index.
pub struct App {
    state: Arc<AppState>,
}

impl App {
    pub fn new(matcher: Matcher) -> App {
        let recorder = PrometheusBuilder::new()
            .set_buckets(LATENCY_BUCKETS)
            .expect("bucket list is not empty")
            .build_recorder();
        let metrics = recorder.handle();
        App { state: Arc::new(AppState { matcher, recorder, metrics }) }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/v1/lookup", get(lookup))
            .route("/healthz", get(healthz))
            .route("/metrics", get(metrics))
            .with_state(self.state.clone())
    }

    /// Serve until `shutdown` resolves, then finish in-flight requests.
    pub async fn serve(
        &self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        // The recorder buffers histogram samples until they are rendered;
        // drain them on a timer so memory stays flat if nothing scrapes /metrics.
        let handle = self.state.metrics.clone();
        let upkeep = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                handle.run_upkeep();
            }
        });
        let result = axum::serve(listener, self.router()).with_graceful_shutdown(shutdown).await;
        upkeep.abort();
        result
    }
}

#[derive(Deserialize)]
struct LookupParams {
    q: Option<String>,
}

fn bad_request(message: &str) -> HttpResponse {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

async fn lookup(
    State(state): State<Arc<AppState>>,
    params: Result<Query<LookupParams>, QueryRejection>,
) -> HttpResponse {
    let q = match params {
        Ok(Query(LookupParams { q: Some(q) })) => q,
        Ok(_) => return bad_request("missing query parameter: q"),
        Err(_) => return bad_request("malformed query string"),
    };
    match state.matcher.lookup_timed(&q) {
        Ok((response, timings)) => {
            state.record(&response, &timings);
            Json(response).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "index read failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "index read failed" })))
                .into_response()
        }
    }
}

impl AppState {
    fn record(&self, response: &Response, timings: &StageTimings) {
        let verdict = if response.is_accepted() { "accepted" } else { "rejected" };
        let reason = response.reason.map_or("none", Reason::as_str);
        let matched_via = response.matched_via.map_or("none", |via| via.as_str());
        metrics::with_local_recorder(&self.recorder, || {
            counter!(LOOKUPS_TOTAL, "verdict" => verdict, "reason" => reason, "matched_via" => matched_via)
                .increment(1);
            histogram!(LOOKUP_SECONDS).record(response.latency_us as f64 / 1e6);
            for (stage, time) in timings.stages() {
                histogram!(STAGE_SECONDS, "stage" => stage).record(time.as_secs_f64());
            }
        });
        if matches!(response.reason, Some(Reason::NotFound | Reason::Ambiguous)) {
            tracing::info!(
                target: MISS_TARGET,
                reason = reason,
                normalized = %response.normalized,
                "miss"
            );
        }
    }
}

async fn healthz(State(state): State<Arc<AppState>>) -> HttpResponse {
    Json(state.matcher.index().manifest()).into_response()
}

async fn metrics(State(state): State<Arc<AppState>>) -> HttpResponse {
    let body = state.metrics.render();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response()
}
