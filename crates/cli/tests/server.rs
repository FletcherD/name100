//! `hundred serve` as a real process on a real socket, queried by `hundred lookup`.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Manifest, Reason, Response};

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

fn hundred() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hundred"));
    // Run somewhere without a hundred.toml, and without inherited overrides.
    command.current_dir(index_dir());
    for (key, _) in std::env::vars() {
        if key.starts_with("HUNDRED_") {
            command.env_remove(key);
        }
    }
    command
}

/// A running server on an ephemeral port; stopped with SIGTERM on drop.
struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(extra_args: &[&str], env: &[(&str, &str)]) -> Server {
        let mut command = hundred();
        command
            .args(["serve", "--index", index_dir().to_str().unwrap(), "--bind", "127.0.0.1:0"])
            .args(extra_args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        stderr.read_line(&mut line).unwrap();
        let url = line
            .trim()
            .strip_prefix("hundred: listening on ")
            .unwrap_or_else(|| panic!("unexpected first stderr line: {line:?}"))
            .to_owned();
        Server { child, url }
    }

    fn lookup(&self, name: &str, extra: &[&str]) -> (i32, String) {
        let output = hundred()
            .args(["lookup", name, "--server", &self.url])
            .args(extra)
            .output()
            .unwrap();
        (output.status.code().unwrap(), String::from_utf8(output.stdout).unwrap())
    }

    fn get(&self, path: &str) -> reqwest::blocking::Response {
        reqwest::blocking::get(format!("{}{path}", self.url)).unwrap()
    }

    /// Ask the server to stop, wait for it, and return its exit code and stdout (the JSON log).
    fn stop(mut self) -> (Option<i32>, String) {
        Command::new("kill").arg(self.child.id().to_string()).status().unwrap();
        let status = self.child.wait().unwrap();
        let mut log = String::new();
        self.child.stdout.take().unwrap().read_to_string(&mut log).unwrap();
        (status.code(), log)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn lookup_through_the_server_matches_the_spec_examples() {
    let server = Server::start(&[], &[]);

    let (code, stdout) = server.lookup("Ruth Bader Ginsberg", &[]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("✓ Ruth Bader Ginsburg (Q11116) — "), "{stdout}");
    assert!(stdout.trim_end().ends_with("[fuzzy, corrected]"), "{stdout}");

    let (code, stdout) = server.lookup("Elon Musk", &[]);
    assert_eq!(code, 1);
    assert_eq!(stdout.trim_end(), "✗ Elon Musk (Q317521) — not a woman: gender is male");

    let (code, stdout) = server.lookup("Beyoncé", &["--json"]);
    assert_eq!(code, 0);
    let response: Response = serde_json::from_str(&stdout).unwrap();
    assert_eq!(response.matched.unwrap().qid, "Q36153");

    // Characters that need URL encoding survive the trip.
    let (code, stdout) = server.lookup("Sinéad O'Connor & co?", &["--json"]);
    assert_eq!(code, 1);
    let response: Response = serde_json::from_str(&stdout).unwrap();
    assert_eq!(response.input, "Sinéad O'Connor & co?");
}

#[test]
fn health_metrics_and_miss_log_work_end_to_end() {
    let server = Server::start(&[], &[]);

    let manifest: Manifest = server.get("/healthz").json().unwrap();
    assert!(manifest.counts.records > 300);
    assert_eq!(server.get("/v1/lookup").status(), 400);

    assert_eq!(server.lookup("Oh fuck off", &[]).0, 1);
    assert_eq!(server.lookup("Jennifer", &[]).0, 1);
    assert_eq!(server.lookup("Cher", &[]).0, 0);

    let metrics = server.get("/metrics").text().unwrap();
    assert!(metrics.contains("hundred_lookups_total{"), "{metrics}");
    assert!(metrics.contains(r#"reason="not_found""#), "{metrics}");
    assert!(metrics.contains("hundred_lookup_stage_seconds_bucket{"), "{metrics}");

    // SIGTERM shuts down cleanly, and the log on stdout is JSON lines with the misses in it.
    let (code, log) = server.stop();
    assert_eq!(code, Some(0));
    let events: Vec<serde_json::Value> =
        log.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let misses: Vec<(&str, &str)> = events
        .iter()
        .filter(|event| event["target"] == "hundred::miss")
        .map(|event| {
            let fields = &event["fields"];
            (fields["reason"].as_str().unwrap(), fields["normalized"].as_str().unwrap())
        })
        .collect();
    assert_eq!(misses, [("not_found", "oh fuck off"), ("ambiguous", "jennifer")]);
    assert!(events.iter().any(|event| event["fields"]["message"] == "index loaded"));

    // The ranking script reads that log.
    let log_path = index_dir().join("server.log");
    std::fs::write(&log_path, &log).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rank_misses.py");
    let ranked = Command::new("python3").arg(script).arg(&log_path).output().unwrap();
    assert!(ranked.status.success());
    assert_eq!(
        String::from_utf8(ranked.stdout).unwrap(),
        "1\tambiguous\tjennifer\n1\tnot_found\toh fuck off\n"
    );
}

#[test]
fn matching_settings_come_from_the_environment_and_a_config_file() {
    // A mononym threshold nobody meets makes "Cher" ambiguous.
    let server = Server::start(&[], &[("HUNDRED_MONONYM_MIN_SITELINKS", "100000")]);
    let (code, stdout) = server.lookup("Cher", &["--json"]);
    assert_eq!(code, 1);
    let response: Response = serde_json::from_str(&stdout).unwrap();
    assert_eq!(response.reason, Some(Reason::Ambiguous));
    drop(server);

    let config: PathBuf = index_dir().join("strict.toml");
    std::fs::write(&config, "fuzzy_min_len = 50\n").unwrap();
    let server = Server::start(&["--config", config.to_str().unwrap()], &[]);
    assert_eq!(server.lookup("Madona", &[]).0, 1, "fuzzy matching is effectively off");
    assert_eq!(server.lookup("Madonna", &[]).0, 0);
}

#[test]
fn category_games_work_through_the_server() {
    let server = Server::start(&[], &[]);
    let (code, stdout) = server.lookup("Tom Hanks", &["--category", "occupation:Q33999"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("✓ Tom Hanks"), "{stdout}");
    let (code, stdout) = server.lookup("Angela Merkel", &["--category", "occupation:Q33999"]);
    assert_eq!(code, 1);
    assert_eq!(stdout.trim_end(), "✗ Angela Merkel (Q567) — not in category: actor");

    let output = hundred()
        .args(["categories", "tennis players", "--server", &server.url])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("tennis player"), "{stdout}");

    // An unknown category is the server's 400, reported as an error.
    let output = hundred()
        .args(["lookup", "Cher", "--server", &server.url, "--category", "occupation:Q1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown category"));
}

#[test]
fn errors_exit_2() {
    // Nothing listens on this port.
    let output =
        hundred().args(["lookup", "Cher", "--server", "http://127.0.0.1:1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not reach the server"));

    let output = hundred().args(["serve"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no index given"));

    let output = hundred()
        .args(["serve", "--index", index_dir().to_str().unwrap()])
        .env("HUNDRED_DOMINANCE_RATIO", "0.2")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("dominance_ratio"));
}
