#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/base/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(not(unix))]
compile_error!("the `base` binary currently supports only unix platforms");

use clap::Parser;

mod app;
mod cli;
mod config;
mod execution;
mod unified;

fn main() {
    base_cli_utils::init_common!();

    if let Err(err) = app::BaseApp::new(cli::BaseCli::parse()).run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
