//! Load test runner binary.

use clap::Parser;

mod cli;

#[tokio::main(flavor = "current_thread")]
async fn main() -> eyre::Result<()> {
    cli::Cli::parse().run().await
}
