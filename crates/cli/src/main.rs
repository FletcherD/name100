use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use hundred_build::{build_index, dump_date_from_path, open_dump, BuildOptions};
use hundred_core::Manifest;

/// Name 100 Women lookup server.
#[derive(Parser)]
#[command(name = "hundred", version)]
struct Cli {
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
        /// Languages whose labels and aliases are indexed.
        #[arg(long, value_delimiter = ',', default_value = "en,mul")]
        langs: Vec<String>,
        /// Minimum number of Wikipedia articles for a person to be kept.
        #[arg(long, default_value_t = 1)]
        min_sitelinks: u32,
        /// Dump date (YYYY-MM-DD) for the manifest. Defaults to the date in the dump's filename.
        #[arg(long)]
        dump_date: Option<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::BuildIndex { dump, out, langs, min_sitelinks, dump_date } => {
            let options = BuildOptions {
                langs,
                min_sitelinks,
                dump_date: dump_date.or_else(|| dump_date_from_path(&dump)),
            };
            let manifest = build_index(open_dump(&dump)?, &out, &options)?;
            print_report(&manifest, &out);
            Ok(())
        }
    }
}

fn print_report(manifest: &Manifest, out: &std::path::Path) {
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
    println!("  artifact size        {:>12.2} MB", mb(s.total));
    println!("    names.fst          {:>12.2} MB", mb(s.names_fst));
    println!("    postings.bin       {:>12.2} MB", mb(s.postings_bin));
    println!("    records_hot.bin    {:>12.2} MB", mb(s.records_hot_bin));
    println!("    records_cold.bin   {:>12.2} MB", mb(s.records_cold_bin));
    println!("  build time           {:>12.2} s", manifest.build_seconds);
    if let Some(rss) = manifest.peak_rss_mb {
        println!("  peak memory          {:>12} MB", rss);
    }
}
