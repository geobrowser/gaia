use actions_indexer_pipeline::consumer::ActionsConsumer;
use actions_indexer_pipeline::loader::ActionsLoader;
use actions_indexer_pipeline::processor::ActionsProcessor;
use actions_indexer_pipeline::consumer::stream::sink::SubstreamsStreamProvider;
use actions_indexer_repository::PostgresActionsRepository;
use std::sync::Arc;
use crate::config::handlers::VoteHandler;
use crate::errors::IndexingError;

// Use CARGO_MANIFEST_DIR to get path relative to the crate
const PKG_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/geo-actions-v0.1.0.spkg");
const MODULE_NAME: &str = "map_actions";

/// `Dependencies` struct holds the necessary components for the action indexer.
///
/// It includes a consumer for ingesting actions, a processor for handling
/// business logic, and a loader for persisting processed data.
pub struct Dependencies {
    pub consumer: Box<ActionsConsumer>,
    pub processor: Box<ActionsProcessor>,
    pub loader: Box<ActionsLoader>,
}

impl Dependencies {
    /// Creates a new `Dependencies` instance.
    ///
    /// This asynchronous function is responsible for initializing and wiring up
    /// all the external services and components required by the indexer.
    ///
    /// # Returns
    ///
    /// A `Result` which is `Ok(Self)` on successful initialization or an
    /// `IndexingError` if any dependency fails to initialize.
    pub async fn new() -> Result<Self, IndexingError> {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = sqlx::PgPool::connect(&database_url).await.map_err(|e| IndexingError::Database(e.into()))?;

        let substreams_endpoint = std::env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT must be set");
        let substreams_api_token = std::env::var("SUBSTREAMS_API_TOKEN").expect("SUBSTREAMS_API_TOKEN must be set");
        let package_file = PKG_FILE.to_string();
        let module_name = MODULE_NAME.to_string();
        let block_range = None;
        let params = vec![];

        let substreams_stream_provider = SubstreamsStreamProvider::new(
            substreams_endpoint,
            package_file,
            module_name,
            block_range,
            params,
            Some(substreams_api_token),
        );

        let actions_consumer = ActionsConsumer::new(Box::new(substreams_stream_provider));
        let mut actions_processor = ActionsProcessor::new();
        actions_processor.register_handler(1, 1, Arc::new(VoteHandler));
        
        let actions_loader = ActionsLoader::new(Arc::new(PostgresActionsRepository::new(pool).await.map_err(|e| IndexingError::Repository(e))?));

        Ok(Dependencies {
            consumer: Box::new(actions_consumer),
            processor: Box::new(actions_processor),
            loader: Box::new(actions_loader),
        })
    }
}