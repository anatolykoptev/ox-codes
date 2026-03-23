mod serve;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ox-codes", version, about = "Rust code search backend")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start HTTP server
    Serve {
        #[arg(long, env = "PORT", default_value = "8902")]
        port: u16,
    },
    /// Show version
    Version,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve { port } => serve::run(port).await,
        Commands::Version => {
            println!("ox-codes {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
