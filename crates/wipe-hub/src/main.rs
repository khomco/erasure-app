use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::warn;
use tracing_subscriber::EnvFilter;
use wipe_hub::{startup_warnings, Args, Config};

#[derive(Parser)]
#[command(name = "wipe-hub", version, about = "Wipestation vendor hub")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the hub's HTTP API.
    Serve(Args),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve(args) => {
            let config = Config::from_args(args)?;
            for w in startup_warnings(&config) {
                warn!("{w}");
            }
            wipe_hub::serve(config).await
        }
    }
}
