//! The HTTP API, exercised in-process against an index built from the fixture.

use std::path::Path;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Index, Manifest, MatchConfig, Matcher, Reason, Response};
use hundred_server::{
    App, ServerOptions, LOOKUPS_TOTAL, LOOKUP_SECONDS, RATE_LIMITED_TOTAL, STAGE_SECONDS,
};
use tower::ServiceExt;

fn index_dir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dump = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.ndjson");
        let dir = tempfile::tempdir().unwrap();
        build_index(open_dump(dump.to_str().unwrap()).unwrap(), dir.path(), &BuildOptions { category_min_people: 5, ..Default::default() })
            .unwrap();
        dir
    })
    .path()
}

/// A fresh app, so metrics start at zero.
fn app() -> App {
    App::new(Matcher::new(Index::open(index_dir()).unwrap(), MatchConfig::default()))
}

async fn get(app: &App, uri: &str) -> (StatusCode, String, String) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let response = app.router().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, String::from_utf8(body.to_vec()).unwrap())
}

async fn lookup(app: &App, query: &str) -> Response {
    let (status, content_type, body) = get(app, &format!("/v1/lookup?q={query}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(content_type, "application/json");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn lookup_returns_200_with_a_verdict_for_acceptances_and_rejections() {
    let app = app();
    let accepted = lookup(&app, "Ruth+Bader+Ginsberg").await;
    assert!(accepted.is_accepted());
    assert_eq!(accepted.input, "Ruth Bader Ginsberg");
    assert_eq!(accepted.matched.unwrap().qid, "Q11116");
    assert_eq!(accepted.corrected_from.as_deref(), Some("ruth bader ginsberg"));
    assert_eq!(accepted.matched_name.as_deref(), Some("Ruth Bader Ginsburg"));

    let (_, _, body) = get(&app, "/v1/lookup?q=Stefani+Germanotta").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["matched_name"], "Stefani Germanotta");
    assert_eq!(json["matched_as"], "alias");
    assert_eq!(json["match"]["name"], "Lady Gaga");

    let rejected = lookup(&app, "Miley%20Cyrus").await;
    assert_eq!(rejected.reason, Some(Reason::NotWoman));
    assert_eq!(rejected.reason_detail.as_deref(), Some("gender is genderfluid"));

    assert_eq!(lookup(&app, "Beyonc%C3%A9").await.matched.unwrap().qid, "Q36153");
    assert_eq!(lookup(&app, "1234").await.reason, Some(Reason::NotAName));
    assert_eq!(lookup(&app, "").await.reason, Some(Reason::NotAName));
    assert_eq!(lookup(&app, "Jennifer").await.candidates.len(), 5);
}

#[tokio::test]
async fn lookup_without_q_is_a_400() {
    let app = app();
    for uri in ["/v1/lookup", "/v1/lookup?name=Cher"] {
        let (status, _, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        let error: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(error["error"], "missing query parameter: q");
    }
    // Nothing was looked up, so nothing was counted.
    let (_, _, metrics) = get(&app, "/metrics").await;
    assert!(!metrics.contains(LOOKUPS_TOTAL), "{metrics}");
}

#[tokio::test]
async fn unknown_paths_are_404() {
    assert_eq!(get(&app(), "/v2/lookup?q=Cher").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn healthz_returns_the_manifest() {
    let (status, content_type, body) = get(&app(), "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let served: Manifest = serde_json::from_str(&body).unwrap();
    let on_disk: Manifest =
        serde_json::from_slice(&std::fs::read(index_dir().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(served, on_disk);
    assert!(served.counts.records > 300);
}

/// The value of the sample line that carries every one of `labels`.
fn sample(metrics: &str, name: &str, labels: &[&str]) -> Option<f64> {
    metrics
        .lines()
        .find(|line| {
            let metric = line.split(['{', ' ']).next().unwrap_or_default();
            metric == name && labels.iter().all(|label| line.contains(label))
        })
        .and_then(|line| line.rsplit(' ').next()?.parse().ok())
}

#[tokio::test]
async fn metrics_count_lookups_by_verdict_and_how_they_matched() {
    let app = app();
    for query in ["Cher", "Madonna", "Madona", "Elon+Musk", "Jennifer", "zzzz+qqqq+xxxx", "1234"] {
        lookup(&app, query).await;
    }
    let (status, content_type, metrics) = get(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/plain"));

    let count = |labels: &[&str]| sample(&metrics, LOOKUPS_TOTAL, labels);
    assert_eq!(count(&[r#"verdict="accepted""#, r#"matched_via="exact""#]), Some(2.0), "{metrics}");
    assert_eq!(count(&[r#"verdict="accepted""#, r#"matched_via="fuzzy""#]), Some(1.0));
    assert_eq!(count(&[r#"reason="not_woman""#, r#"matched_via="exact""#]), Some(1.0));
    assert_eq!(count(&[r#"reason="ambiguous""#]), Some(1.0));
    assert_eq!(count(&[r#"reason="not_found""#, r#"matched_via="none""#]), Some(1.0));
    assert_eq!(count(&[r#"reason="not_a_name""#]), Some(1.0));

    // Whole-lookup histogram saw all seven; each stage saw only the lookups that reached it.
    let seen = |name: &str, labels: &[&str]| sample(&metrics, &format!("{name}_count"), labels);
    assert_eq!(seen(LOOKUP_SECONDS, &[]), Some(7.0));
    assert_eq!(seen(STAGE_SECONDS, &[r#"stage="guard""#]), Some(7.0));
    assert_eq!(seen(STAGE_SECONDS, &[r#"stage="exact""#]), Some(6.0));
    assert_eq!(seen(STAGE_SECONDS, &[r#"stage="fuzzy""#]), Some(2.0));
    assert_eq!(seen(STAGE_SECONDS, &[r#"stage="resolve""#]), Some(5.0));
    assert!(metrics.contains(&format!("{LOOKUP_SECONDS}_bucket{{le=\"0.001\"}}")), "{metrics}");
}

// ---- Cross-origin access and rate limiting ---------------------------------

fn app_with(options: ServerOptions) -> App {
    let matcher = Matcher::new(Index::open(index_dir()).unwrap(), MatchConfig::default());
    App::with_options(matcher, options)
}

/// The `Access-Control-Allow-Origin` header a lookup from `origin` gets back.
async fn allowed_origin(app: &App, origin: &str) -> Option<String> {
    let request = Request::builder()
        .uri("/v1/lookup?q=Cher")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap();
    let response = app.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("access-control-allow-origin")
        .map(|value| value.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn pages_on_this_machine_may_call_the_api_by_default() {
    let app = app();
    for origin in ["http://localhost:5173", "http://127.0.0.1:8000", "http://localhost"] {
        assert_eq!(allowed_origin(&app, origin).await.as_deref(), Some(origin));
    }
    for origin in [
        "https://example.github.io",
        "http://localhost.evil.example",
        "http://localhost:5173.evil.example",
        "https://localhost:5173",
    ] {
        assert_eq!(allowed_origin(&app, origin).await, None, "{origin}");
    }
}

#[tokio::test]
async fn only_configured_origins_may_call_the_api() {
    let app = app_with(ServerOptions {
        cors_origins: vec!["https://example.github.io".into()],
        cors_allow_localhost: false,
        ..Default::default()
    });
    let allowed = "https://example.github.io";
    assert_eq!(allowed_origin(&app, allowed).await.as_deref(), Some(allowed));
    assert_eq!(allowed_origin(&app, "https://other.github.io").await, None);
    assert_eq!(allowed_origin(&app, "http://localhost:5173").await, None);

    let open = app_with(ServerOptions { cors_origins: vec!["*".into()], ..Default::default() });
    assert!(allowed_origin(&open, "https://anywhere.example").await.is_some());
}

#[tokio::test]
async fn lookups_beyond_the_rate_limit_get_429() {
    // 60 a minute allows a burst of 6.
    let app = app_with(ServerOptions { rate_limit_per_minute: 60, ..Default::default() });
    for _ in 0..6 {
        assert_eq!(get(&app, "/v1/lookup?q=Cher").await.0, StatusCode::OK);
    }
    let request = Request::builder().uri("/v1/lookup?q=Cher").body(Body::empty()).unwrap();
    let response = app.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get("retry-after").unwrap(), "1");

    // Health and metrics are not limited, and the refusal is counted.
    assert_eq!(get(&app, "/healthz").await.0, StatusCode::OK);
    let (_, _, metrics) = get(&app, "/metrics").await;
    assert_eq!(sample(&metrics, RATE_LIMITED_TOTAL, &[]), Some(1.0), "{metrics}");
    assert_eq!(sample(&metrics, &format!("{LOOKUP_SECONDS}_count"), &[]), Some(6.0));
}

#[tokio::test]
async fn forwarded_addresses_are_limited_separately_only_when_trusted() {
    let lookup_as = |app: &App, client: &str| {
        let request = Request::builder()
            .uri("/v1/lookup?q=Cher")
            .header("x-forwarded-for", format!("203.0.113.9, {client}"))
            .body(Body::empty())
            .unwrap();
        let router = app.router();
        async move { router.oneshot(request).await.unwrap().status() }
    };

    let trusting = app_with(ServerOptions {
        rate_limit_per_minute: 60,
        trust_forwarded_for: true,
        ..Default::default()
    });
    for _ in 0..6 {
        assert_eq!(lookup_as(&trusting, "198.51.100.1").await, StatusCode::OK);
    }
    assert_eq!(lookup_as(&trusting, "198.51.100.1").await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(lookup_as(&trusting, "198.51.100.2").await, StatusCode::OK);

    // Untrusted: the header is ignored, so both "clients" share one bucket.
    let wary = app_with(ServerOptions { rate_limit_per_minute: 60, ..Default::default() });
    for _ in 0..6 {
        assert_eq!(lookup_as(&wary, "198.51.100.1").await, StatusCode::OK);
    }
    assert_eq!(lookup_as(&wary, "198.51.100.2").await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn no_rate_limit_when_set_to_zero() {
    let app = app_with(ServerOptions { rate_limit_per_minute: 0, ..Default::default() });
    for _ in 0..200 {
        assert_eq!(get(&app, "/v1/lookup?q=Cher").await.0, StatusCode::OK);
    }
}

// ---- Category games -----------------------------------------------------------

#[tokio::test]
async fn categories_are_searchable() {
    let app = app();
    let (status, content_type, body) = get(&app, "/v1/categories?q=actors&limit=3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let listed = json["categories"].as_array().unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0]["id"], "occupation:Q33999");
    assert_eq!(listed[0]["label"], "actor");
    assert_eq!(listed[0]["kind"], "occupation");
    assert!(listed[0]["people"].as_u64().unwrap() > 100);
    assert!(listed[0]["women"].as_u64().unwrap() > 50);

    // No query lists the biggest; the limit is capped.
    let (_, _, body) = get(&app, "/v1/categories").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["categories"].as_array().unwrap().len(), 10);
    assert_eq!(json["categories"][0]["id"], "women");
    let (_, _, body) = get(&app, "/v1/categories?limit=100000").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["categories"].as_array().unwrap().len(), 50);

    let (_, _, body) = get(&app, "/v1/categories?q=zzzz").await;
    assert_eq!(body, r#"{"categories":[]}"#);

    // By id: exactly those, in the order asked, unknown ids left out.
    let actor = listed[0]["id"].as_str().unwrap();
    let (_, _, body) = get(&app, &format!("/v1/categories?ids={actor},nonsense,women")).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let by_id = json["categories"].as_array().unwrap();
    assert_eq!(by_id.len(), 2);
    assert_eq!(by_id[0], listed[0]);
    assert_eq!(by_id[1]["id"], "women");
}

#[tokio::test]
async fn lookups_can_be_played_in_a_category() {
    let app = app();
    let hanks = lookup(&app, "Tom+Hanks&category=occupation:Q33999").await;
    assert!(hanks.is_accepted());
    assert_eq!(hanks.category, ["occupation:Q33999"]);
    // Without a category it is the women game.
    let hanks = lookup(&app, "Tom+Hanks").await;
    assert_eq!(hanks.reason, Some(Reason::NotWoman));
    assert!(hanks.category.is_empty());

    let einstein = lookup(&app, "Albert+Einstein&category=women,occupation:Q901").await;
    assert_eq!(einstein.reason, Some(Reason::NotInCategory));
    assert_eq!(einstein.reason_detail.as_deref(), Some("gender is male"));
    assert!(lookup(&app, "Marie+Curie&category=women,occupation:Q901").await.is_accepted());

    for (query, message) in [
        ("category=occupation:Q1", "unknown category: occupation:Q1"),
        ("category=nonsense", "unknown category: nonsense"),
        ("category=women,men,occupation:Q901", "at most 2 categories can be combined"),
    ] {
        let (status, _, body) = get(&app, &format!("/v1/lookup?q=Cher&{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        let error: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(error["error"], message);
    }
}
