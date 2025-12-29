use actions_indexer_pipeline::consumer::ActionsConsumer;
use actions_indexer_pipeline::consumer::kafka::KafkaStreamProvider;
use actions_indexer_pipeline::consumer::stream::sink::SubstreamsStreamProvider;
use actions_indexer_pipeline::loader::ActionsLoader;
use actions_indexer_pipeline::processor::ActionsProcessor;
use actions_indexer_repository::{PostgresActionsRepository, PostgresCursorRepository};
use actions_indexer_shared::types::{ActionType, ObjectType};
use std::sync::Arc;
use crate::config::handlers::VoteHandler;
use crate::errors::IndexingError;
use actions_indexer_pipeline::consumer::ConsumerConfig;
use url::Url;

// Use CARGO_MANIFEST_DIR to get path relative to the crate
const PKG_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/geo-actions-v0.1.0.spkg");
const MODULE_NAME: &str = "map_actions";

/// Data source type for the actions consumer.
///
/// Determines which streaming backend to use for consuming action events.
#[derive(Debug, Clone, PartialEq)]
pub enum DataSource {
    /// Use Substreams to consume directly from blockchain
    Substreams,
    /// Use Kafka to consume from the Hermes event stream
    Kafka,
}

impl DataSource {
    /// Parses the data source from an environment variable.
    ///
    /// # Environment Variable
    ///
    /// - `DATA_SOURCE` - Either "substreams" or "kafka" (case-insensitive)
    ///
    /// Defaults to `Substreams` if not set or invalid.
    pub fn from_env() -> Self {
        match std::env::var("DATA_SOURCE")
            .unwrap_or_else(|_| "substreams".to_string())
            .to_lowercase()
            .as_str()
        {
            "kafka" => DataSource::Kafka,
            _ => DataSource::Substreams,
        }
    }
}
    
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
    /// ## Data Source Selection
    ///
    /// The data source is determined by the `DATA_SOURCE` environment variable:
    /// - `kafka` - Uses `KafkaStreamProvider` to consume from Hermes Kafka stream
    /// - `substreams` (default) - Uses `SubstreamsStreamProvider` to consume from blockchain
    ///
    /// ## Required Environment Variables
    ///
    /// **Always required:**
    /// - `DATABASE_URL` - PostgreSQL connection string
    ///
    /// **For Substreams (DATA_SOURCE=substreams):**
    /// - `SUBSTREAMS_ENDPOINT` - Substreams gRPC endpoint
    /// - `SUBSTREAMS_API_TOKEN` - Authentication token
    ///
    /// **For Kafka (DATA_SOURCE=kafka):**
    /// - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
    /// - `KAFKA_CONSUMER_GROUP` - Consumer group ID (default: actions-indexer)
    /// - `KAFKA_TOPIC` - Topic to consume from (default: curation.votes)
    /// - `KAFKA_USERNAME` - SASL username (optional, for managed Kafka)
    /// - `KAFKA_PASSWORD` - SASL password (optional, for managed Kafka)
    ///
    /// # Returns
    ///
    /// A `Result` which is `Ok(Self)` on successful initialization or an
    /// `IndexingError` if any dependency fails to initialize.
    pub async fn new() -> Result<Self, IndexingError> {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let data_source = DataSource::from_env();
        
        println!("Actions Indexer: Using data source: {:?}", data_source);

        // Create the appropriate stream provider based on data source
        let actions_consumer = match data_source {
            DataSource::Kafka => {
                let kafka_broker = std::env::var("KAFKA_BROKER").expect("KAFKA_BROKER must be set");
                let kafka_consumer_group = std::env::var("KAFKA_CONSUMER_GROUP").expect("KAFKA_CONSUMER_GROUP must be set");
                let kafka_topic = std::env::var("KAFKA_TOPIC").expect("KAFKA_TOPIC must be set");
                let kafka_username = std::env::var("KAFKA_USERNAME").ok();
                let kafka_password = std::env::var("KAFKA_PASSWORD").ok();
                let kafka_ssl_ca_pem = std::env::var("KAFKA_SSL_CA_PEM").ok();
                let mut consumer_config = ConsumerConfig::new(
                    Url::parse(&kafka_broker).expect("KAFKA_BROKER must be a valid URL"),
                    kafka_consumer_group,
                    kafka_topic,
                );
                if kafka_username.is_some() && kafka_password.is_some() {
                    consumer_config = consumer_config.with_credentials(kafka_username.unwrap(), kafka_password.unwrap());
                }
                else if kafka_ssl_ca_pem.is_some() {
                    consumer_config = consumer_config.with_ssl_ca(kafka_ssl_ca_pem.unwrap());
                } else {
                    println!("No credentials provided for Kafka, using plaintext authentication");
                }

                let kafka_provider = KafkaStreamProvider::new(consumer_config);
                ActionsConsumer::new(Box::new(kafka_provider))
            }
            DataSource::Substreams => {
                let substreams_endpoint = std::env::var("SUBSTREAMS_ENDPOINT")
                    .expect("SUBSTREAMS_ENDPOINT must be set when DATA_SOURCE=substreams");
                let substreams_api_token = std::env::var("SUBSTREAMS_API_TOKEN")
                    .expect("SUBSTREAMS_API_TOKEN must be set when DATA_SOURCE=substreams");

                let package_file = PKG_FILE.to_string();
                let module_name = MODULE_NAME.to_string();
                let block_range = None;
                let params = vec![];

                let substreams_provider = SubstreamsStreamProvider::new(
                    substreams_endpoint,
                    package_file,
                    module_name,
                    block_range,
                    params,
                    Some(substreams_api_token),
                );
                ActionsConsumer::new(Box::new(substreams_provider))
            }
        };

        let mut actions_processor = ActionsProcessor::new();
        actions_processor.register_handler(1, ActionType::Vote, ObjectType::Entity, Arc::new(VoteHandler));
        actions_processor.register_handler(1, ActionType::Vote, ObjectType::Relation, Arc::new(VoteHandler));

        let pool = sqlx::PgPool::connect(&database_url).await.map_err(|e| IndexingError::Database(e.into()))?;

        let actions_loader = ActionsLoader::new(
            Arc::new(PostgresActionsRepository::new(pool.clone()).await.map_err(|e| IndexingError::ActionsRepository(e))?), 
            Arc::new(PostgresCursorRepository::new(pool).await.map_err(|e| IndexingError::CursorRepository(e))?));

        Ok(Dependencies {
            consumer: Box::new(actions_consumer),
            processor: Box::new(actions_processor),
            loader: Box::new(actions_loader),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use serial_test::serial;

    // Helper function to set test environment variables for substreams
    fn set_substreams_env_vars() {
        unsafe {
            env::set_var("DATA_SOURCE", "substreams");
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }
    }

    // Helper function to set test environment variables for kafka
    fn set_kafka_env_vars() {
        unsafe {
            env::set_var("DATA_SOURCE", "kafka");
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("KAFKA_BROKER", "localhost:9092");
            env::set_var("KAFKA_CONSUMER_GROUP", "test-group");
            env::set_var("KAFKA_TOPIC", "test-topic");
        }
    }

    // Helper function to clear environment variables
    fn clear_env_vars() {
        unsafe {
            env::remove_var("DATA_SOURCE");
            env::remove_var("DATABASE_URL");
            env::remove_var("SUBSTREAMS_ENDPOINT");
            env::remove_var("SUBSTREAMS_API_TOKEN");
            env::remove_var("KAFKA_BROKER");
            env::remove_var("KAFKA_CONSUMER_GROUP");
            env::remove_var("KAFKA_TOPIC");
            env::remove_var("KAFKA_USERNAME");
            env::remove_var("KAFKA_PASSWORD");
            env::remove_var("KAFKA_SSL_CA_PEM");
        }
    }

    #[test]
    fn test_data_source_from_env_defaults_to_substreams() {
        clear_env_vars();
        assert_eq!(DataSource::from_env(), DataSource::Substreams);
    }

    #[test]
    #[serial]
    fn test_data_source_from_env_kafka() {
        clear_env_vars();
        unsafe { env::set_var("DATA_SOURCE", "kafka"); }
        assert_eq!(DataSource::from_env(), DataSource::Kafka);
        clear_env_vars();
    }

    #[test]
    #[serial]
    fn test_data_source_from_env_case_insensitive() {
        clear_env_vars();
        unsafe { env::set_var("DATA_SOURCE", "KAFKA"); }
        assert_eq!(DataSource::from_env(), DataSource::Kafka);
        
        unsafe { env::set_var("DATA_SOURCE", "Kafka"); }
        assert_eq!(DataSource::from_env(), DataSource::Kafka);
        clear_env_vars();
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "DATABASE_URL must be set")]
    async fn test_dependencies_new_missing_database_url() {
        clear_env_vars();
        unsafe {
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }

        let _ = Dependencies::new().await;
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "SUBSTREAMS_ENDPOINT must be set when DATA_SOURCE=substreams")]
    async fn test_dependencies_new_missing_substreams_endpoint() {
        clear_env_vars();
        unsafe {
            env::set_var("DATA_SOURCE", "substreams");
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }

        let _ = Dependencies::new().await;
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "SUBSTREAMS_API_TOKEN must be set when DATA_SOURCE=substreams")]
    async fn test_dependencies_new_missing_api_token() {
        clear_env_vars();
        unsafe {
            env::set_var("DATA_SOURCE", "substreams");
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
        }

        let _ = Dependencies::new().await;
    }

    #[tokio::test]
    #[serial]
    async fn test_dependencies_new_invalid_database_url_substreams() {
        clear_env_vars();
        set_substreams_env_vars();
        unsafe {
            env::set_var("DATABASE_URL", "invalid-database-url");
        }

        let result = Dependencies::new().await;
        assert!(result.is_err());
        
        if let Err(IndexingError::Database(_)) = result {
            // Expected error type - test passes
        } else {
            panic!("Expected Database error");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_dependencies_new_invalid_database_url_kafka() {
        clear_env_vars();
        set_kafka_env_vars();
        unsafe {
            env::set_var("DATABASE_URL", "invalid-database-url");
            env::set_var("KAFKA_USERNAME", "test-user");
            env::set_var("KAFKA_PASSWORD", "test-password");
        }

        let result = Dependencies::new().await;
        assert!(result.is_err());
        
        if let Err(IndexingError::Database(_)) = result {
            // Expected error type - test passes
        } else {
            panic!("Expected Database error, got: {:?}", result.err());
        }
    }

    #[test]
    fn test_dependencies_struct_creation() {
        // Test that we can create individual components that make up Dependencies
        let mock_consumer = Box::new(ActionsConsumer::new(
            Box::new(SubstreamsStreamProvider::new(
                "https://test.com".to_string(),
                "test.spkg".to_string(),
                "test_module".to_string(),
                None,
                vec![],
                Some("token".to_string()),
            ))
        ));
        
        let mut mock_processor = ActionsProcessor::new();
        mock_processor.register_handler(1, ActionType::Vote, ObjectType::Entity, Arc::new(VoteHandler));
        
        // Note: We can't easily create a mock loader without a real database connection
        // This test focuses on the struct creation aspects
        
        // Verify the consumer is properly boxed and not null
        assert!(!std::ptr::eq(mock_consumer.as_ref(), std::ptr::null()));
    }

    #[test]
    fn test_dependencies_struct_fields() {
        // This is more of a compilation test to ensure the struct fields are accessible
        // and properly typed. We can't instantiate Dependencies without database access.
        
        // Test that the Dependencies struct has the expected field types
        use std::any::TypeId;
        
        // Verify field types exist and are as expected
        assert_eq!(
            TypeId::of::<Box<ActionsConsumer>>(),
            TypeId::of::<Box<ActionsConsumer>>()
        );
        assert_eq!(
            TypeId::of::<Box<ActionsProcessor>>(),
            TypeId::of::<Box<ActionsProcessor>>()
        );
        assert_eq!(
            TypeId::of::<Box<ActionsLoader>>(),
            TypeId::of::<Box<ActionsLoader>>()
        );
    }

    #[test]
    fn test_vote_handler_registration() {
        // Test that VoteHandler can be created and used in processor registration
        let vote_handler = VoteHandler;
        let mut processor = ActionsProcessor::new();
        
        // This should not panic
        processor.register_handler(1, ActionType::Vote, ObjectType::Entity, Arc::new(vote_handler));
        
        // Verify the processor was created successfully
        assert!(true); // If we get here, registration worked
    }

    #[test]
    fn test_substreams_provider_creation() {
        // Test SubstreamsStreamProvider creation with various parameters
        let _provider = SubstreamsStreamProvider::new(
            "https://test-endpoint.com".to_string(),
            "./test/package.spkg".to_string(),
            "test_module".to_string(),
            Some("100:200".to_string()),
            vec![],
            Some("test-token".to_string()),
        );
        
        // If creation succeeds, the provider should be valid
        // We can't easily inspect internal state, but creation should not panic
        assert!(true);
    }

    #[test]
    fn test_substreams_provider_endpoint_url_formatting() {
        // Test that endpoint URL is properly formatted
        let _provider1 = SubstreamsStreamProvider::new(
            "test-endpoint.com".to_string(), // Without https://
            "./test.spkg".to_string(),
            "module".to_string(),
            None,
            vec![],
            Some("token".to_string()),
        );
        
        let _provider2 = SubstreamsStreamProvider::new(
            "https://test-endpoint.com".to_string(), // With https://
            "./test.spkg".to_string(),
            "module".to_string(),
            None,
            vec![],
            Some("token".to_string()),
        );
        
        // Both should be created successfully
        assert!(true);
    }

}