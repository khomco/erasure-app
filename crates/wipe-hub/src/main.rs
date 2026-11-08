use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::warn;
use tracing_subscriber::EnvFilter;
use wipe_hub::config::{AdminCreateArgs, ReleasePublishArgs};
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
    /// Manage vendor admins (vendor staff who use the portal's admin area).
    Admin {
        #[command(subcommand)]
        cmd: AdminCmd,
    },
    /// Manage releases (station software customers download).
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Create a release, store its files and publish it, in one step.
    Publish(ReleasePublishArgs),
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Create a vendor admin and email them a set-password link.
    Create(AdminCreateArgs),
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
        Cmd::Admin {
            cmd: AdminCmd::Create(args),
        } => {
            let created = wipe_hub::admin::create_vendor_admin(args).await?;
            println!(
                "created vendor admin {} ({})",
                created.email, created.user_id
            );
            match &created.email_sent {
                Ok(()) => println!(
                    "set-password link emailed to {}; it works once and expires in 1 hour",
                    created.email
                ),
                Err(e) => eprintln!(
                    "warning: the set-password email to {} could not be sent ({e}); \
                     the admin should use \"Forgot password\" on the portal",
                    created.email
                ),
            }
            Ok(())
        }
        Cmd::Release {
            cmd: ReleaseCmd::Publish(args),
        } => {
            let release = wipe_hub::release_admin::publish_from_cli(args).await?;
            println!(
                "published release {} ({}) on the {} channel",
                release.version, release.id, release.channel
            );
            for a in &release.artifacts {
                println!(
                    "  {}  {}  {} bytes  sha256 {}",
                    a.platform, a.filename, a.size_bytes, a.sha256
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
