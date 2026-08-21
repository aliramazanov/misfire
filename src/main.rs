use clap::Parser;
use misfire::analyze::{Outcome, analyze};
type Failure = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Failure>;

use misfire::baseline::{self, Baseline};
use misfire::config::Config;
use misfire::git::Repo;
use misfire::observe::{self, LogFormat, Verbosity};
use misfire::rules::Options;
use misfire::{Confidence, Finding};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "misfire",
    version,
    about = "Flags diffs that weaken your test suite"
)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    #[arg(long, default_value = "origin/main")]
    base: String,
    #[arg(long, conflicts_with_all = ["github", "sarif"])]
    json: bool,
    #[arg(long, conflicts_with_all = ["json", "sarif"])]
    github: bool,
    #[arg(long, conflicts_with_all = ["json", "github"])]
    sarif: bool,
    #[arg(
        long,
        value_name = "FILE",
        help = "Suppress findings listed in this baseline"
    )]
    baseline: Option<PathBuf>,
    #[arg(
        long,
        value_name = "FILE",
        num_args = 0..=1,
        default_missing_value = baseline::FILE_NAME,
        help = "Write current findings to a baseline and exit"
    )]
    write_baseline: Option<PathBuf>,
    #[arg(long, value_name = "DIR")]
    path: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "medium")]
    min_confidence: MinConfidence,
    #[arg(
        long,
        help = "Report test files deleted alongside changes to their siblings"
    )]
    file_deleted_rule: bool,
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[arg(long)]
    explain: bool,
    #[arg(long, value_enum, default_value = "text")]
    log_format: LogFormatArg,
    #[arg(long)]
    metrics: bool,
    #[arg(long)]
    timings: bool,
    #[arg(long, value_name = "URL", env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
enum LogFormatArg {
    Text,
    Json,
}

impl From<LogFormatArg> for LogFormat {
    fn from(f: LogFormatArg) -> Self {
        match f {
            LogFormatArg::Text => Self::Text,
            LogFormatArg::Json => Self::Json,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
enum MinConfidence {
    Low,
    Medium,
    High,
}

impl From<MinConfidence> for Confidence {
    fn from(m: MinConfidence) -> Self {
        match m {
            MinConfidence::Low => Self::Low,
            MinConfidence::Medium => Self::Medium,
            MinConfidence::High => Self::High,
        }
    }
}

#[cfg(feature = "otel")]
fn start_otlp(endpoint: Option<&str>) -> Option<observe::otlp::Pipeline> {
    let endpoint = endpoint?;
    match observe::otlp::Pipeline::install(endpoint) {
        Ok(pipeline) => Some(pipeline),
        Err(e) => {
            eprintln!("misfire: could not start OTLP export to {endpoint}: {e}");
            None
        }
    }
}

#[cfg(feature = "otel")]
fn finish_otlp(pipeline: Option<observe::otlp::Pipeline>) {
    if let Some(p) = pipeline {
        p.shutdown();
    }
}

#[cfg(not(feature = "otel"))]
fn start_otlp(endpoint: Option<&str>) -> Option<()> {
    if endpoint.is_some() {
        eprintln!("misfire: --otlp-endpoint needs a build with the `otel` feature");
    }
    None
}

#[cfg(not(feature = "otel"))]
#[allow(clippy::needless_pass_by_value)]
fn finish_otlp(_pipeline: Option<()>) {}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.explain {
        Verbosity::Trace
    } else {
        Verbosity::from_occurrences(cli.verbose)
    };

    observe::install(verbosity, cli.log_format.into());

    let otlp = start_otlp(cli.otlp_endpoint.as_deref());

    let outcome = run(&cli);
    finish_otlp(otlp);

    match outcome {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            report_failure(&e);
            ExitCode::from(2)
        }
    }
}

fn report_failure(error: &Failure) {
    eprint!("misfire: {error}");
    let mut cause = error.source();
    while let Some(next) = cause {
        eprint!(": {next}");
        cause = next.source();
    }
    eprintln!();

    let Some(misfire) = error.downcast_ref::<misfire::error::Error>() else {
        return;
    };

    if !misfire.is_shallow_checkout_problem() {
        return;
    }

    eprintln!(
        "\n  This usually means a shallow checkout: actions/checkout clones with\n\
         \x20 fetch-depth: 1, so the base commit was never fetched.\n\n\
         \x20 Fix it with `fetch-depth: 0` on actions/checkout, or fetch the ref:\n\
         \x20   git fetch --no-tags origin <branch>:refs/remotes/origin/<branch>\n\n\
         \x20 misfire will not guess at a diff it cannot see, so it stops here rather\n\
         \x20 than reporting a clean result it has not earned."
    );
}

fn run(cli: &Cli) -> Result<usize> {
    let repo = open_repository(cli)?;
    let base = resolve_base(cli, &repo)?;
    let cfg = Config::load(repo.root())?;

    let outcome = examine_change(&repo, &base, &cfg, cli)?;
    report_diagnostics(cli, &outcome);

    let findings = select_findings(cli, outcome.findings)?;
    let Some(findings) = findings else {
        return Ok(0);
    };

    emit(cli, &findings)?;

    let floor: Confidence = cli.min_confidence.into();
    Ok(findings.iter().filter(|f| f.confidence >= floor).count())
}

fn open_repository(cli: &Cli) -> Result<Repo> {
    let cwd = match &cli.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };

    Ok(Repo::discover(&cwd)?)
}

fn resolve_base(cli: &Cli, repo: &Repo) -> Result<String> {
    let phase = observe::Phase::start("resolve-base");
    let base = repo.merge_base(&cli.base)?;
    phase.done(&base);

    Ok(base)
}

fn examine_change(repo: &Repo, base: &str, cfg: &Config, cli: &Cli) -> Result<Outcome> {
    let mut rules = cfg.rule_set();

    if cli.file_deleted_rule {
        rules = rules.with(misfire::Rule::TestFileDeleted);
    }

    let opts = Options {
        rules,
        confidence: cfg.confidence_overrides(),
    };

    let started = std::time::Instant::now();
    let outcome = analyze(repo, base, cfg, &opts)?;

    tracing::info!(
        findings = outcome.findings.len(),
        unanalysed = outcome.unanalysed.len(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "analysis complete"
    );

    outcome.metrics.emit();

    Ok(outcome)
}

fn report_diagnostics(cli: &Cli, outcome: &Outcome) {
    if cli.metrics {
        match serde_json::to_string_pretty(&outcome.metrics.to_json()) {
            Ok(body) => eprintln!("{body}"),
            Err(e) => eprintln!("misfire: could not render metrics: {e}"),
        }
    }

    if cli.timings {
        eprint!("{}", outcome.metrics.timings_table());
    }

    if outcome.unanalysed.is_empty() {
        return;
    }

    for path in &outcome.unanalysed {
        eprintln!("misfire: could not read {path}, so it was not checked");
    }

    eprintln!(
        "misfire: {} test{} skipped. This is usually a gap in the grammar rather than a \
         problem with the code, most often a raw string containing a '#' inside a macro. \
         misfire reports them rather than guessing at them.",
        outcome.unanalysed.len(),
        if outcome.unanalysed.len() == 1 {
            ""
        } else {
            "s"
        }
    );
}

fn select_findings(cli: &Cli, found: Vec<Finding>) -> Result<Option<Vec<Finding>>> {
    let enabled = found;

    if let Some(path) = &cli.write_baseline {
        let recorded = Baseline::from_findings(&enabled);
        let n = recorded.entries.len();

        recorded.write(path)?;

        eprintln!(
            "misfire: wrote {n} baseline entr{} to {}",
            if n == 1 { "y" } else { "ies" },
            path.display()
        );

        return Ok(None);
    }

    Ok(Some(match &cli.baseline {
        Some(path) => baseline::filter(enabled, &Baseline::load(path)?),
        None => enabled,
    }))
}

fn emit(cli: &Cli, findings: &[Finding]) -> Result<()> {
    if cli.github {
        print!("{}", misfire::report::github_annotations(findings));
    } else if cli.sarif {
        println!(
            "{}",
            serde_json::to_string_pretty(&misfire::sarif::build(findings))?
        );
    } else if cli.json {
        println!("{}", serde_json::to_string_pretty(&json(findings))?);
    } else {
        print!("{}", misfire::report::text(findings));
    }

    Ok(())
}

fn json(findings: &[Finding]) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "findings": findings.iter().map(|f| serde_json::json!({
            "file": f.file,
            "line": f.line,
            "rule": f.rule.id(),
            "test": f.test,
            "confidence": format!("{:?}", f.confidence).to_lowercase(),
            "detail": f.detail,
        })).collect::<Vec<_>>(),
    })
}
