use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

mod commands;
mod embedding_slots;
mod opensearch_client;

use commands::{
    add_embedding_slot::AddEmbeddingSlotCommand, backfill_name_raw::BackfillNameRawCommand,
    create::CreateIndexCommand, delete::DeleteIndexCommand,
    ensure_search_pipeline::EnsureSearchPipelineCommand, eval_slot::EvalSlotCommand,
    full_migration::FullMigrationCommand, list::ListIndicesCommand, list_slots::ListSlotsCommand,
    reindex::ReindexCommand, retire_embedding_slot::RetireEmbeddingSlotCommand,
    set_default_slot::SetDefaultSlotCommand, update_alias::UpdateAliasCommand,
};

/// Get the prefixed alias name based on environment.
///
/// - `staging` → `staging_{base_alias}`
/// - `testnet` → `testnet_{base_alias}`
/// - `production` (or any other value) → `{base_alias}`
fn get_prefixed_alias(environment: &str, base_alias: &str) -> String {
    match environment {
        "staging" => format!("staging_{}", base_alias),
        "testnet" => format!("testnet_{}", base_alias),
        _ => base_alias.to_string(),
    }
}

#[derive(Parser)]
#[command(name = "search-admin")]
#[command(about = "CLI tool for managing OpenSearch indices", long_about = None)]
struct Cli {
    /// OpenSearch URL
    #[arg(long, env = "OPENSEARCH_URL", default_value = "http://localhost:9200")]
    opensearch_url: String,

    /// Base index alias name (will be prefixed based on environment)
    #[arg(long, env = "INDEX_ALIAS", default_value = "entities")]
    index_alias: String,

    /// Environment (staging, testnet, or production). Required. staging/testnet add a "staging_"/"testnet_" prefix to index names.
    #[arg(long, env = "ENVIRONMENT")]
    environment: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new versioned index
    CreateIndex(CreateIndexCommand),

    /// Reindex data from source to target version
    Reindex(ReindexCommand),

    /// Delete an index version
    DeleteIndex(DeleteIndexCommand),

    /// List all indices and aliases
    ListIndices(ListIndicesCommand),

    /// Update alias to point to a new index version
    UpdateAlias(UpdateAliasCommand),

    /// Run full migration workflow (create, stop, reindex, update alias, start)
    FullMigration(FullMigrationCommand),

    /// Backfill name_raw field from existing name values
    BackfillNameRaw(BackfillNameRawCommand),

    /// Register an embedding slot: its k-NN fields and descriptor in the index _meta
    AddEmbeddingSlot(AddEmbeddingSlotCommand),

    /// Make a registered embedding slot the index default
    SetDefaultSlot(SetDefaultSlotCommand),

    /// Remove an embedding slot from the index _meta (its fields stay until the next migration)
    RetireEmbeddingSlot(RetireEmbeddingSlotCommand),

    /// Show embedding slots, the default, coverage, and whether the service has them loaded
    ListSlots(ListSlotsCommand),

    /// Create or update the hybrid (lexical + k-NN) search pipeline for this alias
    EnsureSearchPipeline(EnsureSearchPipelineCommand),

    /// Evaluate an embedding slot on a golden query set: recall@k, MRR, contrastive pairs, floor
    EvalSlot(EvalSlotCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    // Validate environment value
    if cli.environment != "staging"
        && cli.environment != "testnet"
        && cli.environment != "production"
    {
        error!(
            environment = %cli.environment,
            "ENVIRONMENT must be 'staging', 'testnet' or 'production'"
        );
        std::process::exit(1);
    }

    // Sanitize OpenSearch URL to avoid logging credentials
    let sanitized_url = if let Ok(parsed) = url::Url::parse(&cli.opensearch_url) {
        // Create URL without userinfo (username:password)
        let mut sanitized = parsed.clone();
        sanitized.set_username("").ok();
        sanitized.set_password(None).ok();
        sanitized.to_string()
    } else {
        // If parsing fails, just log a placeholder
        "[invalid-url]".to_string()
    };

    // Apply environment prefix to index alias
    let index_alias = get_prefixed_alias(&cli.environment, &cli.index_alias);

    info!(
        environment = %cli.environment,
        "=== ENVIRONMENT: {} ===",
        cli.environment
    );

    info!(
        opensearch_url = %sanitized_url,
        base_alias = %cli.index_alias,
        index_alias = %index_alias,
        "Starting search-admin CLI"
    );

    let result = match cli.command {
        Commands::CreateIndex(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::Reindex(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::DeleteIndex(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::ListIndices(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::UpdateAlias(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::FullMigration(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::BackfillNameRaw(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::AddEmbeddingSlot(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::SetDefaultSlot(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::RetireEmbeddingSlot(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::ListSlots(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::EnsureSearchPipeline(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
        Commands::EvalSlot(cmd) => cmd.execute(&cli.opensearch_url, &index_alias).await,
    };

    if let Err(ref e) = result {
        error!(error = %e, "Command failed");
        std::process::exit(1);
    }

    Ok(())
}
