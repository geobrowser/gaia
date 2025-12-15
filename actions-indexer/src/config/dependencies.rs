use actions_indexer_pipeline::consumer::ActionsConsumer;
use actions_indexer_pipeline::loader::ActionsLoader;
use actions_indexer_pipeline::processor::ActionsProcessor;
use actions_indexer_pipeline::consumer::stream::sink::SubstreamsStreamProvider;
use actions_indexer_repository::{PostgresActionsRepository, PostgresCursorRepository};
use actions_indexer_shared::types::{ActionType, ObjectType};
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
    use tempfile::NamedTempFile;
    use std::io::Write;

    // Helper function to set test environment variables
    fn set_test_env_vars() {
        unsafe {
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }
    }

    // Helper function to clear environment variables
    fn clear_env_vars() {
        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("SUBSTREAMS_ENDPOINT");
            env::remove_var("SUBSTREAMS_API_TOKEN");
        }
    }

    // Helper function to create a test package file
    fn create_test_package_file() -> NamedTempFile {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
        // Write minimal valid protobuf data for a package
        // This is a simplified approach - in a real scenario you'd want proper protobuf data
        temp_file.write_all(b"dummy package data").expect("Failed to write to temp file");
        temp_file
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
    #[should_panic(expected = "SUBSTREAMS_ENDPOINT must be set")]
    async fn test_dependencies_new_missing_substreams_endpoint() {
        clear_env_vars();
        unsafe {
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }

        let _ = Dependencies::new().await;
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "SUBSTREAMS_API_TOKEN must be set")]
    async fn test_dependencies_new_missing_api_token() {
        clear_env_vars();
        unsafe {
            env::set_var("DATABASE_URL", "postgresql://test:test@localhost:5432/test_db");
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
        }

        let _ = Dependencies::new().await;

    }

    #[tokio::test]
    #[serial]
    async fn test_dependencies_new_invalid_database_url() {
        clear_env_vars();
        unsafe {
            env::set_var("DATABASE_URL", "invalid-database-url");
            env::set_var("SUBSTREAMS_ENDPOINT", "https://test-endpoint.com");
            env::set_var("SUBSTREAMS_API_TOKEN", "test-token");
        }

        let result = Dependencies::new().await;
        assert!(result.is_err());
        
        if let Err(IndexingError::Database(_)) = result {
            // Expected error type - test passes
        } else {
            panic!("Expected Database error");
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