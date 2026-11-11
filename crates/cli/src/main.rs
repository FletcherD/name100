mod config;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use hundred_build::{build_index, dump_date_from_path, open_dump, BuildOptions};
use hundred_core::catalogue::MAX_TERMS;
use hundred_core::{Index, Manifest, Matcher, Response, Term};
use hundred_server::App;

use config::Config;

/// Name 100 Women lookup server.
#[derive(Parser)]
#[command(name = "hundred", version)]
struct Cli {
    /// Config file. Defaults to ./hundred.toml if it exists. Every key can
    /// also be set as HUNDRED_<KEY> in the environment.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build an index artifact from a Wikidata entity dump.
    BuildIndex {
        /// Dump to read: .json.bz2, .json.gz, plain NDJSON, or `-` for stdin.
        #[arg(long)]
        dump: String,
        /// Directory to write the artifact into.
        #[arg(long)]
        out: PathBuf,
        /// Languages whose labels and aliases are indexed [default: en,mul].
        #[arg(long, value_delimiter = ',')]
        langs: Option<Vec<String>>,
        /// Minimum number of Wikipedia articles for a person to be kept [default: 1].
        #[arg(long)]
        min_sitelinks: Option<u32>,
        /// Articles a person needs before a misspelling is corrected to them [default: 10].
        #[arg(long)]
        fuzzy_min_sitelinks: Option<u32>,
        /// People a category needs before it is offered as a game [default: 100].
        #[arg(long)]
        category_min_people: Option<u32>,
        /// Dump date (YYYY-MM-DD) for the manifest. Defaults to the date in the dump's filename.
        #[arg(long)]
        dump_date: Option<String>,
    },
    /// Load an index and serve lookups over HTTP.
    Serve {
        /// Index directory to serve.
        #[arg(long)]
        index: Option<PathBuf>,
        /// Address to listen on [default: 127.0.0.1:8080].
        #[arg(long)]
        bind: Option<String>,
    },
    /// Decide whether a name is a real, notable woman.
    ///
    /// Asks a running server, or with --local looks the name up in-process.
    /// Exits 0 if accepted, 1 if rejected, 2 on error.
    Lookup {
        /// The name to look up.
        name: String,
        /// Server to ask.
        #[arg(long, default_value = "http://127.0.0.1:8080", conflicts_with = "local")]
        server: String,
        /// Look the name up in-process instead of asking a server.
        #[arg(long)]
        local: bool,
        /// Index directory, for --local.
        #[arg(long, requires = "local")]
        index: Option<PathBuf>,
        /// Play a category instead of naming women: up to two category
        /// ids, comma separated, as listed by `hundred categories`.
        #[arg(long, value_delimiter = ',')]
        category: Vec<String>,
        /// Print the full JSON response instead of one line.
        #[arg(long)]
        json: bool,
    },
    /// Search the categories that can be played ("actor", "France", "tennis").
    Categories {
        /// Text to search for; omit to list the biggest categories.
        query: Option<String>,
        /// How many to list.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Server to ask.
        #[arg(long, default_value = "http://127.0.0.1:8080", conflicts_with = "local")]
        server: String,
        /// Search in-process instead of asking a server.
        #[arg(long)]
        local: bool,
        /// Index directory, for --local.
        #[arg(long, requires = "local")]
        index: Option<PathBuf>,
        /// Print JSON instead of one line per category.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let config = Config::load(cli.config.as_deref())?;
    match cli.command {
        Command::BuildIndex {
            dump,
            out,
            langs,
            min_sitelinks,
            fuzzy_min_sitelinks,
            category_min_people,
            dump_date,
        } => {
            let options = BuildOptions {
                langs: langs.unwrap_or_else(|| config.langs.clone()),
                min_sitelinks: min_sitelinks.unwrap_or(config.min_sitelinks),
                fuzzy_min_sitelinks: fuzzy_min_sitelinks.unwrap_or(config.fuzzy_min_sitelinks),
                category_min_people: category_min_people.unwrap_or(config.category_min_people),
                dump_date: dump_date.or_else(|| dump_date_from_path(&dump)),
            };
            let manifest = build_index(open_dump(&dump)?, &out, &options)?;
            print_report(&manifest, &out);
            Ok(ExitCode::SUCCESS)
        }
        Command::Serve { index, bind } => {
            let index = index_path(index, &config)?;
            let bind = bind.unwrap_or_else(|| config.bind.clone());
            serve(&index, &bind, &config)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Lookup { name, server, local, index, category, json } => {
            let response = if local {
                let index = Index::open(&index_path(index, &config)?)?;
                let terms = parse_category(&category, &index)?;
                Matcher::new(index, config.matching()).lookup_in(&name, &terms)?
            } else {
                ask_server(&server, &name, &category)?
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("{}", response.summary_line());
            }
            Ok(if response.is_accepted() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }

        Command::Categories { query, limit, server, local, index, json } => {
            let query = query.unwrap_or_default();
            let listed: Vec<serde_json::Value> = if local {
                let index = Index::open(&index_path(index, &config)?)?;
                let found = index.catalogue().search(&query, limit);
                found.into_iter().map(hundred_server::category_json).collect()
            } else {
                let limit = limit.to_string();
                let reply: serde_json::Value = get_json(
                    &server,
                    "/v1/categories",
                    &[("q", query.as_str()), ("limit", limit.as_str())],
                )?;
                reply["categories"].as_array().cloned().unwrap_or_default()
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&listed)?);
            } else {
                for category in &listed {
                    println!(
                        "{:<32} {:<26} {:>9} people, {:>8} women",
                        category["label"].as_str().unwrap_or_default(),
                        category["id"].as_str().unwrap_or_default(),
                        category["people"],
                        category["women"],
                    );
                }
            }
            Ok(if listed.is_empty() { ExitCode::from(1) } else { ExitCode::SUCCESS })
        }
    }
}

/// The terms named by `--category`, each of which must be in the index's catalogue.
fn parse_category(ids: &[String], index: &Index) -> Result<Vec<Term>> {
    if ids.len() > MAX_TERMS {
        bail!("at most {MAX_TERMS} categories can be combined");
    }
    ids.iter()
        .map(|id| {
            Term::parse(id)
                .filter(|term| index.catalogue().get(term).is_some())
                .ok_or_else(|| anyhow!("unknown category: {id} (try `hundred categories <text>`)"))
        })
        .collect()
}

fn index_path(flag: Option<PathBuf>, config: &Config) -> Result<PathBuf> {
    flag.or_else(|| config.index.clone())
        .ok_or_else(|| anyhow!("no index given: pass --index <dir> or set `index` in the config"))
}

fn serve(index_path: &Path, bind: &str, config: &Config) -> Result<()> {
    // JSON logs on stdout; RUST_LOG adjusts the level.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().json().with_env_filter(filter).with_writer(std::io::stdout).init();

    let started = Instant::now();
    let index = Index::open(index_path)?;
    let manifest = index.manifest().clone();
    let app = App::with_options(Matcher::new(index, config.matching()), config.server());
    tracing::info!(
        index = %index_path.display(),
        records = manifest.counts.records,
        names = manifest.counts.names,
        build_date = %manifest.build_date,
        load_ms = started.elapsed().as_millis() as u64,
        "index loaded"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .with_context(|| format!("binding {bind}"))?;
        let address = listener.local_addr()?;
        // Plain line on stderr, so scripts can find the port when binding to :0.
        eprintln!("hundred: listening on http://{address}");
        tracing::info!(%address, "listening");
        app.serve(listener, shutdown_signal()).await?;
        tracing::info!("shut down");
        Ok(())
    })
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

fn ask_server(server: &str, name: &str, category: &[String]) -> Result<Response> {
    let category = category.join(",");
    let mut query = vec![("q", name)];
    if !category.is_empty() {
        query.push(("category", category.as_str()));
    }
    get_json(server, "/v1/lookup", &query)
}

fn get_json<T: serde::de::DeserializeOwned>(
    server: &str,
    path: &str,
    query: &[(&str, &str)],
) -> Result<T> {
    let url = format!("{}{path}", server.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let reply = client.get(&url).query(query).send().with_context(|| {
        format!("could not reach the server at {server} (is `hundred serve` running?)")
    })?;
    let status = reply.status();
    if !status.is_success() {
        bail!("server at {server} answered {status}: {}", reply.text().unwrap_or_default().trim());
    }
    reply.json().with_context(|| format!("unexpected response from {server}"))
}

fn print_report(manifest: &Manifest, out: &Path) {
    let c = &manifest.counts;
    let s = &manifest.sizes;
    let mb = |bytes: u64| bytes as f64 / 1e6;
    println!("index written to {}", out.display());
    println!("  entities read        {:>12}", c.entities_read);
    println!("  records kept         {:>12}", c.records);
    println!("    woman              {:>12}", c.women);
    println!("    not_woman          {:>12}", c.not_women);
    println!("    fictional          {:>12}", c.fictional);
    println!("  dropped");
    println!("    not a person       {:>12}", c.dropped_not_person);
    println!("    below min sitelinks{:>12}", c.dropped_below_min_sitelinks);
    println!("    no indexed name    {:>12}", c.dropped_no_name);
    println!("    parse errors       {:>12}", c.parse_errors);
    println!("  names                {:>12}", c.names);
    println!("    shared             {:>12}", c.shared_names);
    println!("    typo-correctable   {:>12}", c.fuzzy_names);
    println!("  attribute labels     {:>12}", c.labels);
    println!("  occupations          {:>12}", c.occupations);
    println!("    with a category    {:>12}", c.occupations_categorized);
    println!("  playable categories  {:>12}", c.catalogue_entries);
    println!("  artifact size        {:>12.2} MB", mb(s.total));
    println!("    names.fst          {:>12.2} MB", mb(s.names_fst));
    println!("    names_fuzzy.fst    {:>12.2} MB", mb(s.names_fuzzy_fst));
    println!("    postings.bin       {:>12.2} MB", mb(s.postings_bin));
    println!("    records_hot.bin    {:>12.2} MB", mb(s.records_hot_bin));
    println!("    records_cold.bin   {:>12.2} MB", mb(s.records_cold_bin));
    println!("  build time           {:>12.2} s", manifest.build_seconds);
    if let Some(rss) = manifest.peak_rss_mb {
        println!("  peak memory          {:>12} MB", rss);
    }
}
