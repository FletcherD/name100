//! The lookup server: an axum app over one in-memory index.
//!
//! - `GET /v1/lookup?q=<name>`: the verdict as JSON. Always 200 for a
//!   verdict, including rejections; 400 when `q` is missing.
//! - `GET /v1/categories?q=<text>`: playable categories matching the text.
//! - `GET /healthz`: the loaded index's manifest.
//! - `GET /metrics`: Prometheus text.
//!
//! The game's page is served from another origin, so `/v1/lookup` answers
//! cross-origin requests from the configured origins and is rate limited per
//! client address.

mod limit;

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, Extensions, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response as HttpResponse};
use axum::routing::get;
use axum::{Json, Router};
use hundred_core::catalogue::MAX_TERMS;
use hundred_core::{Entry, Matcher, Reason, Response, StageTimings, Term};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, PrometheusRecorder};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};

use limit::RateLimiter;

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

/// Lookups refused by the rate limit.
pub const RATE_LIMITED_TOTAL: &str = "hundred_rate_limited_total";

/// Who may call the server, and how often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    /// Origins whose pages may call the API from a browser, written as the
    /// browser sends them: `https://example.github.io`. `*` allows any.
    pub cors_origins: Vec<String>,
    /// Also allow pages served from this machine (`http://localhost:<port>`
    /// and `http://127.0.0.1:<port>`), for development.
    pub cors_allow_localhost: bool,
    /// Lookups one client address may make per minute; 0 for no limit.
    /// Short bursts of up to a tenth of this are let through at once.
    pub rate_limit_per_minute: u32,
    /// Take the client address from the `X-Forwarded-For` header. Turn on
    /// only behind a reverse proxy that sets it, or clients can forge it.
    pub trust_forwarded_for: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        ServerOptions {
            cors_origins: Vec::new(),
            cors_allow_localhost: true,
            rate_limit_per_minute: 600,
            trust_forwarded_for: false,
        }
    }
}

impl ServerOptions {
    fn allows_origin(&self, origin: &str) -> bool {
        if self.cors_origins.iter().any(|allowed| allowed == "*" || allowed == origin) {
            return true;
        }
        self.cors_allow_localhost && is_localhost_origin(origin)
    }
}

/// `http://localhost`, `http://127.0.0.1` or `http://[::1]`, with any port.
fn is_localhost_origin(origin: &str) -> bool {
    let Some(host_port) = origin.strip_prefix("http://") else {
        return false;
    };
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
        host_port.strip_prefix(host).is_some_and(|rest| {
            rest.is_empty()
                || rest.strip_prefix(':').is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                })
        })
    })
}

/// Histogram bucket bounds, in seconds: 1 µs to 10 ms, since lookups are
/// expected to take microseconds.
const LATENCY_BUCKETS: &[f64] = &[
    1e-6, 2.5e-6, 5e-6, 1e-5, 2.5e-5, 5e-5, 1e-4, 2.5e-4, 5e-4, 1e-3, 2.5e-3, 5e-3, 1e-2,
];

struct AppState {
    matcher: Matcher,
    options: ServerOptions,
    limiter: Option<RateLimiter>,
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
    /// An app with the default [`ServerOptions`].
    pub fn new(matcher: Matcher) -> App {
        App::with_options(matcher, ServerOptions::default())
    }

    pub fn with_options(matcher: Matcher, options: ServerOptions) -> App {
        let recorder = PrometheusBuilder::new()
            .set_buckets(LATENCY_BUCKETS)
            .expect("bucket list is not empty")
            .build_recorder();
        let metrics = recorder.handle();
        let limiter = RateLimiter::per_minute(options.rate_limit_per_minute);
        App { state: Arc::new(AppState { matcher, options, limiter, recorder, metrics }) }
    }

    pub fn router(&self) -> Router {
        let state = self.state.clone();
        let cors = CorsLayer::new()
            .allow_methods([Method::GET])
            .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
                origin.to_str().is_ok_and(|origin| state.options.allows_origin(origin))
            }));
        Router::new()
            .route("/v1/lookup", get(lookup).layer(cors.clone()))
            .route("/v1/categories", get(categories).layer(cors))
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
        let service = self.router().into_make_service_with_connect_info::<SocketAddr>();
        let result = axum::serve(listener, service).with_graceful_shutdown(shutdown).await;
        upkeep.abort();
        result
    }
}

#[derive(Deserialize)]
struct LookupParams {
    q: Option<String>,
    /// Up to two category ids, comma separated: `women,occupation:Q33999`.
    category: Option<String>,
}

#[derive(Deserialize)]
struct CategoryParams {
    #[serde(default)]
    q: String,
    limit: Option<usize>,
    /// Category ids, comma separated: lists exactly those instead of searching.
    ids: Option<String>,
}

/// Categories listed by `/v1/categories` when `limit` is not given, and at most.
const DEFAULT_CATEGORY_LIMIT: usize = 10;
const MAX_CATEGORY_LIMIT: usize = 50;

fn bad_request(message: &str) -> HttpResponse {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

/// The address to rate limit by. Without connection info (as when a test
/// calls the router directly) every request counts as one client.
fn client_address(state: &AppState, peer: Option<SocketAddr>, headers: &HeaderMap) -> IpAddr {
    let forwarded = || {
        // The last entry is the one our own proxy appended.
        let value = headers.get("x-forwarded-for")?.to_str().ok()?;
        value.rsplit(',').next()?.trim().parse::<IpAddr>().ok()
    };
    state
        .options
        .trust_forwarded_for
        .then(forwarded)
        .flatten()
        .or(peer.map(|address| address.ip()))
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

async fn lookup(
    State(state): State<Arc<AppState>>,
    extensions: Extensions,
    headers: HeaderMap,
    params: Result<Query<LookupParams>, QueryRejection>,
) -> HttpResponse {
    if let Some(refusal) = state.rate_limit(&extensions, &headers) {
        return refusal;
    }
    let (q, category) = match params {
        Ok(Query(LookupParams { q: Some(q), category })) => (q, category),
        Ok(_) => return bad_request("missing query parameter: q"),
        Err(_) => return bad_request("malformed query string"),
    };
    let category = match state.parse_category(category.as_deref()) {
        Ok(category) => category,
        Err(message) => return bad_request(&message),
    };
    match state.matcher.lookup_timed(&q, &category) {
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

async fn categories(
    State(state): State<Arc<AppState>>,
    extensions: Extensions,
    headers: HeaderMap,
    params: Result<Query<CategoryParams>, QueryRejection>,
) -> HttpResponse {
    if let Some(refusal) = state.rate_limit(&extensions, &headers) {
        return refusal;
    }
    let Ok(Query(params)) = params else {
        return bad_request("malformed query string");
    };
    let limit = params.limit.unwrap_or(DEFAULT_CATEGORY_LIMIT).min(MAX_CATEGORY_LIMIT);
    let catalogue = state.matcher.index().catalogue();
    let found: Vec<_> = match &params.ids {
        // A game linked to by id needs its categories' names and sizes back.
        // Ids the catalogue does not hold are left out.
        Some(ids) => ids
            .split(',')
            .filter_map(|id| catalogue.get(&Term::parse(id.trim())?))
            .take(MAX_CATEGORY_LIMIT)
            .collect(),
        None => catalogue.search(&params.q, limit),
    };
    let listed: Vec<_> = found.into_iter().map(category_json).collect();
    Json(json!({ "categories": listed })).into_response()
}

/// A catalogue entry as the API lists it.
pub fn category_json(entry: &Entry) -> serde_json::Value {
    json!({
        "id": entry.term.id(),
        "label": entry.label(),
        "kind": entry.term.kind(),
        "people": entry.people,
        "women": entry.women,
    })
}

impl AppState {
    /// Count this request against the client's allowance; the refusal to
    /// send if it is spent.
    fn rate_limit(&self, extensions: &Extensions, headers: &HeaderMap) -> Option<HttpResponse> {
        let limiter = self.limiter.as_ref()?;
        let peer = extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(address)| *address);
        let retry_after = limiter.check(client_address(self, peer, headers)).err()?;
        metrics::with_local_recorder(&self.recorder, || {
            counter!(RATE_LIMITED_TOTAL).increment(1);
        });
        let seconds = retry_after.as_secs_f64().ceil().max(1.0) as u64;
        Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, seconds.to_string())],
                Json(json!({ "error": "rate limit exceeded", "retry_after_seconds": seconds })),
            )
                .into_response(),
        )
    }

    /// The terms of a `category` parameter, each of which must be in the catalogue.
    fn parse_category(&self, param: Option<&str>) -> Result<Vec<Term>, String> {
        let ids: Vec<&str> =
            param.unwrap_or("").split(',').map(str::trim).filter(|id| !id.is_empty()).collect();
        if ids.len() > MAX_TERMS {
            return Err(format!("at most {MAX_TERMS} categories can be combined"));
        }
        let catalogue = self.matcher.index().catalogue();
        ids.into_iter()
            .map(|id| {
                Term::parse(id)
                    .filter(|term| catalogue.get(term).is_some())
                    .ok_or_else(|| format!("unknown category: {id}"))
            })
            .collect()
    }

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
