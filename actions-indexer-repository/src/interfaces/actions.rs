//! Repository interface for actions, user votes, and vote counts data storage.
//!
//! This module defines the `ActionsRepository` trait, which provides a clean abstraction
//! over the underlying data store for the actions indexer system. It handles the persistence
//! and retrieval of:
//!
//! - **Actions**: Processed blockchain actions (e.g., voting actions)
//! - **User Votes**: Individual user voting records with timestamps
//! - **Vote Counts**: Aggregated vote tallies per entity and space
//! - **Changesets**: Atomic batches of related data modifications
//!
//! The trait is designed to support transactional operations and efficient batch processing,
//! making it suitable for high-throughput blockchain data indexing scenarios.
use crate::errors::ActionsRepositoryError;
use actions_indexer_shared::types::{
    Action, Changeset, UserVote, VoteCountCriteria, VoteCriteria, VotesCount,
};

/// Repository interface for managing actions indexer data storage operations.
///
/// This trait defines the contract for data persistence in the actions indexer system.
/// Implementors provide concrete storage backends (e.g., PostgreSQL, MongoDB) that handle:
///
/// - **Batch Operations**: Efficient insertion and updates for high-throughput scenarios
/// - **Transactional Safety**: Atomic operations to maintain data consistency
/// - **Query Operations**: Retrieval of user votes and vote counts with flexible criteria
/// - **Changeset Management**: Coordinated persistence of related data modifications
///
/// All operations are asynchronous to support non-blocking I/O in concurrent environments.
/// Error handling is centralized through `ActionsRepositoryError` for consistent error reporting.
#[async_trait::async_trait]
pub trait ActionsRepository: Send + Sync {
    /// Inserts a batch of processed actions into the repository.
    ///
    /// This method performs a bulk insertion of action data that has been processed
    /// from blockchain events. Actions represent structured data such as voting actions
    /// that have been validated and are ready for persistence.
    ///
    /// # Arguments
    ///
    /// * `actions` - A slice of `Action` objects to be inserted. Each action contains
    ///   structured data representing a specific blockchain action (e.g., vote).
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all actions were successfully inserted
    /// * `Err(ActionsRepositoryError)` - If the insertion fails due to database errors,
    ///   constraint violations, or connection issues
    ///
    /// # Performance
    ///
    /// This method is optimized for batch operations and should be preferred over
    /// individual insertions for better performance in high-throughput scenarios.
    async fn insert_actions(&self, actions: &[Action]) -> Result<(), ActionsRepositoryError>;

    /// Updates or inserts user vote records in the repository.
    ///
    /// This method performs upsert operations on user voting data, updating existing
    /// records or inserting new ones as needed. Each vote record associates a user
    /// with an entity in a specific space, including the vote type and timestamp.
    ///
    /// # Arguments
    ///
    /// * `user_votes` - A slice of `UserVote` objects to be updated/inserted. Each vote
    ///   contains user_id (voter address), entity_id (what was voted on), space_id
    ///   (voting context), vote_type (e.g., upvote/downvote), and timestamp.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all user votes were successfully updated/inserted
    /// * `Err(ActionsRepositoryError)` - If the operation fails due to database errors,
    ///   invalid data, or connection issues
    ///
    /// # Behavior
    ///
    /// The implementation should handle conflicts gracefully, typically by updating
    /// existing votes with new data rather than creating duplicates.
    async fn update_user_votes(
        &self,
        user_votes: &[UserVote],
    ) -> Result<(), ActionsRepositoryError>;

    /// Updates aggregated vote count records in the repository.
    ///
    /// This method updates the tallied vote counts for entities within specific spaces.
    /// Vote counts represent the aggregated upvotes and downvotes for each entity,
    /// providing efficient access to voting statistics without querying individual votes.
    ///
    /// # Arguments
    ///
    /// * `votes_counts` - A slice of `VotesCount` objects to be updated. Each record
    ///   contains entity_id (the voted-upon entity), space_id (voting context),
    ///   upvotes count, and downvotes count.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all vote counts were successfully updated
    /// * `Err(ActionsRepositoryError)` - If the operation fails due to database errors,
    ///   data inconsistencies, or connection issues
    ///
    /// # Performance
    ///
    /// This method is critical for maintaining up-to-date vote statistics and should
    /// be called whenever individual votes are modified to keep counts synchronized.
    async fn update_votes_counts(
        &self,
        votes_counts: &[VotesCount],
    ) -> Result<(), ActionsRepositoryError>;

    /// Atomically persists a complete changeset to the repository.
    ///
    /// This method handles the transactional persistence of related data modifications
    /// as a single atomic operation. A changeset bundles new actions, updated user votes,
    /// and updated vote counts together, ensuring data consistency across all related
    /// tables/collections.
    ///
    /// # Arguments
    ///
    /// * `changeset` - A reference to the `Changeset` object containing:
    ///   - `actions`: New actions to be inserted
    ///   - `user_votes`: User vote records to be updated/inserted
    ///   - `votes_count`: Aggregated vote counts to be updated
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the entire changeset was successfully persisted
    /// * `Err(ActionsRepositoryError)` - If any part of the changeset fails, ensuring
    ///   rollback of all operations to maintain data integrity
    ///
    /// # Transaction Safety
    ///
    /// This method should implement proper transaction boundaries to ensure that
    /// either all changes succeed or all are rolled back on failure.
    async fn persist_changeset(
        &self,
        changeset: &Changeset<'_>,
    ) -> Result<(), ActionsRepositoryError>;

    /// Retrieves user votes matching the specified criteria.
    ///
    /// This method queries for user vote records based on combinations of user address,
    /// entity ID, and space address. It supports batch queries for efficient retrieval
    /// of multiple vote records in a single operation.
    ///
    /// # Arguments
    ///
    /// * `vote_criteria` - A slice of `VoteCriteria` tuples to query for. Each criterion
    ///   is a tuple containing:
    ///   - `UserAddress` - The blockchain address of the voting user
    ///   - `EntityId` - The UUID of the entity that was voted on
    ///   - `SpaceAddress` - The blockchain address of the space context
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<UserVote>)` - A vector containing all matching vote records. Returns
    ///   an empty vector if no votes match any of the criteria.
    /// * `Err(ActionsRepositoryError)` - If the query fails due to database errors
    ///   or connection issues
    ///
    /// # Performance
    ///
    /// This method is optimized for batch queries and should be preferred when
    /// retrieving multiple vote records simultaneously.
    async fn get_user_votes(
        &self,
        vote_criteria: &[VoteCriteria],
    ) -> Result<Vec<UserVote>, ActionsRepositoryError>;

    /// Retrieves aggregated vote counts for the specified entities and spaces.
    ///
    /// This method queries for vote count records that contain the tallied upvotes
    /// and downvotes for entities within specific spaces. It provides efficient access
    /// to voting statistics without the need to aggregate individual vote records.
    ///
    /// # Arguments
    ///
    /// * `vote_criteria` - A slice of `VoteCountCriteria` tuples to query for. Each
    ///   criterion is a tuple containing:
    ///   - `EntityId` - The UUID of the entity to get vote counts for
    ///   - `SpaceAddress` - The blockchain address of the space context
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<VotesCount>)` - A vector containing all matching vote count records.
    ///   Returns an empty vector if no vote counts match any of the criteria.
    /// * `Err(ActionsRepositoryError)` - If the query fails due to database errors
    ///   or connection issues
    ///
    /// # Performance
    ///
    /// This method provides O(1) access to vote statistics and should be used when
    /// displaying vote counts to users or performing vote-based calculations.
    async fn get_vote_counts(
        &self,
        vote_criteria: &[VoteCountCriteria],
    ) -> Result<Vec<VotesCount>, ActionsRepositoryError>;

    /// Checks if the tables are created in the database.
    ///
    /// This method checks if the tables are created in the database.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - If the tables are created
    /// * `Err(ActionsRepositoryError)` - If the query fails due to database errors
    ///   or connection issues
    async fn check_tables_created(&self) -> Result<bool, ActionsRepositoryError>;
}
