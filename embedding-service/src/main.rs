use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use embedding_service::bundle_cmd;
use embedding_service::config::ServeArgs;
use embedding_service::server::{AppState, Limits, load_slots, serve};

#[derive(Parser)]
#[command(
    name = "embedding-service",
    about = "gaia's in-cluster embedding runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Load bundles and serve /embed, /info and /health (the default).
    Serve(ServeArgs),
    /// Work with model bundles.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },
}

#[derive(Subcommand)]
enum BundleCommand {
    /// Download the artifacts a descriptor pins into <out>/<slot>/ and verify them.
    Fetch {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Hash every artifact in a bundle directory against its descriptor.
    Verify { dir: PathBuf },
    /// Print the slot id a descriptor hashes to.
    Slot { spec: PathBuf },
}

fn init_tracing() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    if std::env::var("LOG_FORMAT").is_ok_and(|v| v == "json") {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli
        .command
        .unwrap_or_else(|| Command::Serve(ServeArgs::parse_from(std::env::args().take(1))))
    {
        Command::Serve(args) => {
            let slots = load_slots(&args)?;
            let state = Arc::new(AppState::new(slots, Limits::from(&args)));
            serve(state, &args.bind, args.port).await
        }
        Command::Bundle { command } => match command {
            BundleCommand::Fetch { spec, out } => bundle_cmd::fetch(&spec, &out).await,
            BundleCommand::Verify { dir } => bundle_cmd::verify(&dir),
            BundleCommand::Slot { spec } => bundle_cmd::slot(&spec),
        },
    }
}
