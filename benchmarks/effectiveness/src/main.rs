//! `effectiveness` — run the retrieval-quality harness and write a dated report.
//!
//! ```text
//! cargo run --bin effectiveness -- [--dataset PATH] [--out DIR] [--label LABEL]
//! ```
//!
//! Defaults: dataset `data/fixtures_v1.json` (relative to the crate), output
//! directory `results/`, label from `$GIT_SHA` or `"local"`. The report is
//! printed as a summary table to stdout and written to
//! `<out>/<timestamp>-<label>.json`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use tinycortex_bench_effectiveness::backend::InMemoryBackend;
use tinycortex_bench_effectiveness::dataset::Dataset;
use tinycortex_bench_effectiveness::harness::{self, HarnessConfig, RunReport};

/// Parsed command-line options.
struct Args {
    dataset: PathBuf,
    out_dir: PathBuf,
    label: String,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut dataset = crate_dir.join("data/fixtures_v1.json");
        let mut out_dir = crate_dir.join("results");
        let mut label = std::env::var("GIT_SHA").unwrap_or_else(|_| "local".to_string());

        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--dataset" => {
                    dataset = args.next().context("--dataset needs a path")?.into();
                }
                "--out" => {
                    out_dir = args.next().context("--out needs a path")?.into();
                }
                "--label" => {
                    label = args.next().context("--label needs a value")?;
                }
                "-h" | "--help" => {
                    println!("usage: effectiveness [--dataset PATH] [--out DIR] [--label LABEL]");
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }

        Ok(Self {
            dataset,
            out_dir,
            label,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    let dataset = Dataset::from_json_path(&args.dataset)?;
    let config = HarnessConfig::default();

    let backend = InMemoryBackend::new();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let report = harness::run(
        &backend,
        &dataset,
        &config,
        timestamp.clone(),
        args.label.clone(),
    )
    .await?;

    print_summary(&report);

    let out_path = write_report(&args.out_dir, &report)?;
    println!("\nwrote {}", out_path.display());
    Ok(())
}

/// Render the aggregate as a compact stdout table.
fn print_summary(report: &RunReport) {
    let agg = &report.aggregate;
    println!("TinyCortex retrieval effectiveness");
    println!("  dataset : {}", report.dataset);
    println!("  backend : {}", report.backend);
    println!("  label   : {}", report.label);
    println!("  queries : {}", agg.query_count);
    println!();
    println!(
        "  {:<14} {:>8} {:>10} {:>8}",
        "k", "recall", "precision", "hit"
    );
    for (&k, &recall) in &agg.mean_recall_at_k {
        let precision = agg.mean_precision_at_k.get(&k).copied().unwrap_or(0.0);
        let hit = agg.mean_hit_at_k.get(&k).copied().unwrap_or(0.0);
        println!(
            "  @{:<13} {:>8.3} {:>10.3} {:>8.3}",
            k, recall, precision, hit
        );
    }
    println!();
    println!("  MRR      : {:.3}", agg.mrr);
    println!("  nDCG@{:<3} : {:.3}", report.ndcg_k, agg.mean_ndcg);
}

/// Serialize `report` to `<out_dir>/<timestamp>-<label>.json`, creating the
/// directory if needed. The timestamp is filesystem-sanitized (`:` → `-`).
fn write_report(out_dir: &Path, report: &RunReport) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let stamp = report.timestamp.replace(':', "-");
    let file = out_dir.join(format!("{stamp}-{}.json", report.label));
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(&file, json).with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
}
