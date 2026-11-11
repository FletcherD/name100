//! The HTTP API, exercised in-process against an index built from the fixture.

use std::path::Path;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Index, Manifest, MatchConfig, Matcher, Reason, Response};
use hundred_server::{App, LOOKUPS_TOTAL, LOOKUP_SECONDS, STAGE_SECONDS};
use tower::ServiceExt;

fn index_dir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dump = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.ndjson");
        let dir = tempfile::tempdir().unwrap();
        build_index(open_dump(dump.to_str().unwrap()).unwrap(), dir.path(), &BuildOptions::default())
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
