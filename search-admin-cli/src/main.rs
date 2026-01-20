use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod commands;
mod opensearch_client;

use commands::{
    create::CreateIndexCommand, delete::DeleteIndexCommand, list::ListIndicesCommand,
    monitor::MonitorReindexCommand, reindex::ReindexCommand,
};

#[derive(Parser)]
#[command(name = "search-admin")]
#[command(about = "CLI tool for managing OpenSearch indices", long_about = None)]
struct Cli {
    /// OpenSearch URL
    #[arg(
        long,
        env = "OPENSEARCH_URL",
        default_value = "http://localhost:9200"
    )]
    opensearch_url: String,

    /// Index alias name
    #[arg(long, env = "INDEX_ALIAS", default_value = "entities")]
    index_alias: String,

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

    /// Monitor a reindex task
    MonitorReindex(MonitorReindexCommand),

    /// List all indices and aliases
    ListIndices(ListIndicesCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    info!(
        opensearch_url = %cli.opensearch_url,
        index_alias = %cli.index_alias,
        "Starting search-admin CLI"
    );

    let result = match cli.command {
        Commands::CreateIndex(cmd) => {
            cmd.execute(&cli.opensearch_url, &cli.index_alias).await
        }
        Commands::Reindex(cmd) => cmd.execute(&cli.opensearch_url, &cli.index_alias).await,
        Commands::DeleteIndex(cmd) => {
            cmd.execute(&cli.opensearch_url, &cli.index_alias).await
        }
        Commands::MonitorReindex(cmd) => cmd.execute(&cli.opensearch_url).await,
        Commands::ListIndices(cmd) => cmd.execute(&cli.opensearch_url, &cli.index_alias).await,
    };

    if let Err(ref e) = result {
        error!(error = %e, "Command failed");
        std::process::exit(1);
    }

    Ok(())
}
