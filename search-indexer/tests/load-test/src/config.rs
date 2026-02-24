use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "load-test", about = "Search-indexer load test (100K+ events)")]
pub struct LoadTestConfig {
    /// Kafka broker address
    #[arg(long, default_value = "localhost:9092")]
    pub broker: String,

    /// OpenSearch URL
    #[arg(long, default_value = "http://localhost:9200")]
    pub opensearch_url: String,

    /// RNG seed for reproducibility
    #[arg(long, default_value = "42")]
    pub seed: u64,

    /// Scale multiplier (1.0 = ~100K events, 0.1 = ~10K)
    #[arg(long, default_value = "1.0")]
    pub scale: f64,

    /// Generate and send events, skip validation
    #[arg(long)]
    pub send_only: bool,

    /// Skip sending, validate existing data
    #[arg(long)]
    pub validate_only: bool,

    /// OpenSearch index alias base name (prefixed based on ENVIRONMENT)
    #[arg(long, default_value = "entities")]
    pub index: String,

    /// Max wait seconds for indexer processing
    #[arg(long, default_value = "120")]
    pub timeout: u64,

    /// Enable debug logging
    #[arg(short, long)]
    pub debug: bool,
}

impl LoadTestConfig {
    /// Scale an instance count by the scale factor, with a minimum of 1.
    pub fn scaled(&self, base: usize) -> usize {
        ((base as f64 * self.scale).round() as usize).max(1)
    }

    /// Get the resolved index name with environment prefix applied.
    pub fn resolved_index(&self) -> String {
        match std::env::var("ENVIRONMENT").as_deref() {
            Ok("staging") => format!("staging_{}", self.index),
            Ok("production") => self.index.clone(),
            Ok(other) => panic!("Invalid ENVIRONMENT: {}", other),
            Err(_) => panic!("ENVIRONMENT must be set"),
        }
    }
}
