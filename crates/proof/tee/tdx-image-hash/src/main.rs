#![doc = include_str!("../README.md")]

use base_proof_tee_tdx_image_hash::Cli;
use clap::Parser as _;

#[tokio::main]
async fn main() {
    if let Err(error) = Cli::parse().run().await {
        eprintln!("Error: {error:?}");
        std::process::exit(1);
    }
}
