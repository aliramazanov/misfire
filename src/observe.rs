use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Quiet,

    Verbose,

    Trace,
}

impl Verbosity {
    #[must_use]
    pub fn from_occurrences(n: u8) -> Self {
        match n {
            0 => Self::Quiet,
            1 => Self::Verbose,
            _ => Self::Trace,
        }
    }

    fn directive(self) -> &'static str {
        match self {
            Self::Quiet => "misfire=warn",
            Self::Verbose => "misfire=debug",
            Self::Trace => "misfire=trace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,

    Json,
}

pub fn install(verbosity: Verbosity, format: LogFormat) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("MISFIRE_LOG")
        .unwrap_or_else(|_| EnvFilter::new(verbosity.directive()));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false);

    match format {
        LogFormat::Json => builder.json().flatten_event(true).init(),
        LogFormat::Text => builder.without_time().with_level(true).init(),
    }
}

#[derive(Debug)]
pub struct Phase {
    name: &'static str,
    started: Instant,
}

impl Phase {
    #[must_use]
    pub fn start(name: &'static str) -> Self {
        tracing::debug!(phase = name, "start");
        Self {
            name,
            started: Instant::now(),
        }
    }

    #[must_use]
    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    pub fn done(self, detail: &str) {
        tracing::debug!(
            phase = self.name,
            elapsed_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            detail,
            "done"
        );
    }
}

#[derive(Debug, Default)]
pub struct Metrics {
    pub paths_changed: u64,

    pub files_analysed: u64,

    pub files_unreadable: u64,

    pub tests_indexed: u64,

    pub tests_unreadable: u64,

    pub blobs_read: u64,

    pub helper_dirs_loaded: u64,

    pub findings_by_rule: BTreeMap<String, u64>,

    pub phase_ms: BTreeMap<String, f64>,
}

impl Metrics {
    pub fn record_phase(&mut self, name: &str, elapsed: Duration) {
        *self.phase_ms.entry(name.to_string()).or_default() += elapsed.as_secs_f64() * 1000.0;
    }

    pub fn record_finding(&mut self, rule_id: &str) {
        *self
            .findings_by_rule
            .entry(rule_id.to_string())
            .or_default() += 1;
    }

    #[must_use]
    pub fn findings(&self) -> u64 {
        self.findings_by_rule.values().sum()
    }

    pub fn emit(&self) {
        tracing::info!(
            paths_changed = self.paths_changed,
            files_analysed = self.files_analysed,
            files_unreadable = self.files_unreadable,
            tests_indexed = self.tests_indexed,
            tests_unreadable = self.tests_unreadable,
            blobs_read = self.blobs_read,
            helper_dirs_loaded = self.helper_dirs_loaded,
            findings = self.findings(),
            "run metrics"
        );

        for (rule, count) in &self.findings_by_rule {
            tracing::info!(rule = rule.as_str(), count, "findings by rule");
        }

        for (phase, ms) in &self.phase_ms {
            tracing::info!(phase = phase.as_str(), elapsed_ms = ms, "phase total");
        }
    }

    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "paths_changed": self.paths_changed,
            "files_analysed": self.files_analysed,
            "files_unreadable": self.files_unreadable,
            "tests_indexed": self.tests_indexed,
            "tests_unreadable": self.tests_unreadable,
            "blobs_read": self.blobs_read,
            "helper_dirs_loaded": self.helper_dirs_loaded,
            "findings": self.findings(),
            "findings_by_rule": self.findings_by_rule,
            "phase_ms": self.phase_ms,
        })
    }

    #[must_use]
    pub fn timings_table(&self) -> String {
        use std::fmt::Write;
        let mut rows: Vec<(&String, &f64)> = self.phase_ms.iter().collect();
        rows.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

        let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let mut out = String::from("\n  timings\n");

        for (name, ms) in rows {
            let _ = writeln!(out, "    {name:width$}  {ms:>8.2} ms");
        }

        out
    }
}

#[cfg(feature = "otel")]
pub mod otlp {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig as _;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    #[derive(Debug)]
    pub struct Pipeline {
        tracer: SdkTracerProvider,
        runtime: tokio::runtime::Runtime,
    }

    impl Pipeline {
        pub fn install(endpoint: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()?;
            let guard = runtime.enter();

            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;

            let resource = Resource::builder()
                .with_attributes([
                    KeyValue::new("service.name", "misfire"),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ])
                .build();

            let tracer = SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource)
                .build();

            let layer = tracing_opentelemetry::layer().with_tracer(tracer.tracer("misfire"));
            tracing_subscriber::registry().with(layer).try_init().ok();

            drop(guard);
            Ok(Self { tracer, runtime })
        }

        pub fn shutdown(self) {
            let Self { tracer, runtime } = self;
            let guard = runtime.enter();
            if let Err(e) = tracer.force_flush() {
                eprintln!("misfire: could not flush traces: {e}");
            }
            if let Err(e) = tracer.shutdown() {
                eprintln!("misfire: could not shut down the tracer: {e}");
            }
            drop(guard);

            runtime.shutdown_timeout(std::time::Duration::from_secs(3));
        }
    }
}
