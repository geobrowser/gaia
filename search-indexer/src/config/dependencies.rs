//! Dependency initialization and wiring for the search indexer.

use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use hermes_instrumentation::{info, warn};
use search_indexer_shared::{get_consumer_group_prefix, get_index_prefix};

use rdkafka::admin::AdminClient;
use rdkafka::client::DefaultClientContext;

use crate::consumer::kafka_config::create_client_config;
use crate::consumer::{EntitiesConsumer, ScoresConsumer};
use crate::loader::SearchLoader;
use crate::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::processor::Processor;
use crate::IndexingError;
use search_indexer_repository::opensearch::IndexConfig;
use search_indexer_repository::{OpenSearchProvider, SearchIndexProvider};

/// Default OpenSearch URL.
const DEFAULT_OPENSEARCH_URL: &str = "http://localhost:9200";

/// Default Kafka broker address.
const DEFAULT_KAFKA_BROKER: &str = "localhost:9092";

/// Default Kafka consumer group ID for entities.
const DEFAULT_KAFKA_GROUP_EDITS_ID: &str = "search-indexer-group-edits";

/// Default Kafka consumer group ID for scores.
const DEFAULT_KAFKA_GROUP_SCORES_ID: &str = "search-indexer-group-scores";

/// Default connection retry interval in seconds.
const DEFAULT_RETRY_INTERVAL_SECS: u64 = 15;

/// Connection mode for OpenSearch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    /// Fail immediately if connection fails.
    FailFast,
    /// Retry connection every 15 seconds until successful.
    Retry,
}

/// Container for all initialized dependencies.
pub struct Dependencies {
    /// The configured orchestrator ready to run.
    pub orchestrator: Orchestrator,
    /// The search index provider (for health checks).
    pub provider: Arc<dyn SearchIndexProvider>,
    /// Kafka admin client (for health checks).
    pub kafka_admin: Arc<AdminClient<DefaultClientContext>>,
}

impl ConnectionMode {
    /// Parse connection mode from environment variable.
    ///
    /// Valid values: "fail-fast" or "retry" (case-insensitive)
    /// Defaults to "retry" if not set or invalid.
    fn from_env() -> Self {
        match env::var("OPENSEARCH_CONNECTION_MODE")
            .unwrap_or_else(|_| "retry".to_string())
            .to_lowercase()
            .as_str()
        {
            "fail-fast" | "failfast" | "fail_fast" => Self::FailFast,
            "retry" => Self::Retry,
            _ => {
                warn!("Invalid OPENSEARCH_CONNECTION_MODE, defaulting to 'retry'");
                Self::Retry
            }
        }
    }
}

impl Dependencies {
    /// Initialize all dependencies from environment variables.
    ///
    /// # Environment Variables
    ///
    /// - `OPENSEARCH_URL`: OpenSearch server URL (default: http://localhost:9200)
    /// - `INDEX_ALIAS`: Index alias name (default: "entities")
    /// - `ENTITIES_INDEX_VERSION`: Index version number (default: 0)
    /// - `KAFKA_BROKER`: Kafka broker address (default: localhost:9092)
    /// - `KAFKA_GROUP_ID`: Consumer group ID (default: search-indexer)
    /// - `OPENSEARCH_CONNECTION_MODE`: Connection mode - "fail-fast" or "retry" (default: retry)
    /// - `OPENSEARCH_RETRY_INTERVAL_SECS`: Retry interval in seconds (default: 15)
    ///
    /// # Returns
    ///
    /// * `Ok(Dependencies)` - Initialized dependencies
    /// * `Err(IndexingError)` - If initialization fails (only in fail-fast mode)
    pub async fn new() -> Result<Self, IndexingError> {
        let opensearch_url =
            env::var("OPENSEARCH_URL").unwrap_or_else(|_| DEFAULT_OPENSEARCH_URL.to_string());
        let kafka_broker =
            env::var("KAFKA_BROKER").unwrap_or_else(|_| DEFAULT_KAFKA_BROKER.to_string());

        // Apply environment prefix to Kafka group IDs for staging isolation
        let consumer_group_prefix = get_consumer_group_prefix();
        let base_kafka_group_edits_id =
            env::var("KAFKA_GROUP_EDITS_ID").unwrap_or_else(|_| DEFAULT_KAFKA_GROUP_EDITS_ID.to_string());
        let base_kafka_group_scores_id =
            env::var("KAFKA_GROUP_SCORES_ID").unwrap_or_else(|_| DEFAULT_KAFKA_GROUP_SCORES_ID.to_string());
        let kafka_group_edits_id = format!("{}{}", consumer_group_prefix, base_kafka_group_edits_id);
        let kafka_group_scores_id = format!("{}{}", consumer_group_prefix, base_kafka_group_scores_id);

        let connection_mode = ConnectionMode::from_env();
        let retry_interval = env::var("OPENSEARCH_RETRY_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RETRY_INTERVAL_SECS);

        info!(
            opensearch_url = %opensearch_url,
            kafka_broker = %kafka_broker,
            kafka_group_edits_id = %kafka_group_edits_id,
            kafka_group_scores_id = %kafka_group_scores_id,
            connection_mode = ?connection_mode,
            retry_interval_secs = retry_interval,
            "Initializing dependencies"
        );

        // Get index configuration from environment variables or use defaults
        // Apply environment prefix to index alias for staging isolation
        let index_prefix = get_index_prefix();
        let base_index_alias = env::var("INDEX_ALIAS").unwrap_or_else(|_| "entities".to_string());
        let index_alias = format!("{}{}", index_prefix, base_index_alias);
        let index_version = env::var("ENTITIES_INDEX_VERSION")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let index_config = IndexConfig::new(index_alias.clone(), index_version);

        info!(
            index_alias = %index_alias,
            index_prefix = %index_prefix,
            "Index configuration with environment prefix"
        );

        // Initialize OpenSearch provider with retry logic
        let search_provider = Self::connect_to_opensearch(
            &opensearch_url,
            index_config,
            connection_mode,
            Duration::from_secs(retry_interval),
        )
        .await?;

        info!("OpenSearch connection established");

        // Ensure index and alias exist (validate and create if not exists)
        // Exits if index and alias cannot be created
        search_provider
            .ensure_index_exists()
            .await
            .map_err(|e| IndexingError::config(format!("Failed to ensure index exists: {}", e)))?;

        // Create Kafka admin client for health checks (uses edits group ID)
        let kafka_admin_config = create_client_config(&kafka_broker, &kafka_group_edits_id);
        let kafka_admin: AdminClient<DefaultClientContext> =
            kafka_admin_config.create().map_err(|e| {
                IndexingError::config(format!("Failed to create Kafka admin client: {}", e))
            })?;
        let kafka_admin = Arc::new(kafka_admin);

        info!("Kafka admin client created");

        // Initialize Kafka consumer for entity events
        let entities_consumer =
            EntitiesConsumer::new(&kafka_broker, &kafka_group_edits_id).map_err(|e| {
                IndexingError::config(format!("Failed to create entities consumer: {}", e))
            })?;

        info!("Entities consumer created");

        // Initialize processor
        let processor = Processor::new();

        // Wrap provider in Arc for sharing between loader and health checks
        let provider = Arc::new(search_provider);

        // Initialize loader with search provider
        let loader = SearchLoader::new(provider.clone());

        // Initialize Kafka consumer for score updates
        let scores_consumer = ScoresConsumer::new(&kafka_broker, &kafka_group_scores_id).map_err(|e| {
            IndexingError::config(format!("Failed to create scores consumer: {}", e))
        })?;

        info!("Scores consumer created");

        let orchestrator_config = OrchestratorConfig::from_env();
        info!(
            channel_buffer_size = orchestrator_config.channel_buffer_size,
            "Orchestrator config from env"
        );
        let orchestrator = Orchestrator::with_config(
            Arc::new(entities_consumer),
            Arc::new(scores_consumer),
            processor,
            loader,
            orchestrator_config,
        );

        Ok(Self {
            orchestrator,
            provider,
            kafka_admin,
        })
    }

    /// Connect to OpenSearch with retry logic based on connection mode.
    async fn connect_to_opensearch(
        url: &str,
        index_config: IndexConfig,
        mode: ConnectionMode,
        retry_interval: Duration,
    ) -> Result<OpenSearchProvider, IndexingError> {
        loop {
            match Self::try_connect_opensearch(url, index_config.clone()).await {
                Ok(provider) => return Ok(provider),
                Err(e) => match mode {
                    ConnectionMode::FailFast => {
                        return Err(IndexingError::config(format!(
                            "Failed to connect to OpenSearch: {}",
                            e
                        )));
                    }
                    ConnectionMode::Retry => {
                        warn!(
                            opensearch_url = %url,
                            error = %e,
                            retry_interval_secs = retry_interval.as_secs(),
                            "Failed to connect to OpenSearch, retrying..."
                        );
                        sleep(retry_interval).await;
                    }
                },
            }
        }
    }

    /// Attempt to connect to OpenSearch.
    async fn try_connect_opensearch(
        url: &str,
        index_config: IndexConfig,
    ) -> Result<OpenSearchProvider, IndexingError> {
        // Initialize OpenSearch provider
        let search_provider = OpenSearchProvider::new(url, index_config)
            .await
            .map_err(|e| {
                IndexingError::config(format!("Failed to create OpenSearch provider: {}", e))
            })?;

        Ok(search_provider)
    }
}
