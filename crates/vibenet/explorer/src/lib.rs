//! vibescan: a minimal block explorer for vibenet.
//!
//! The node already answers every read-by-hash/number query over JSON-RPC.
//! We only persist the one thing it cannot: the address -> activity index.
//! Everything else (block bodies, receipts, logs, balances, code, storage)
//! is fetched from the upstream RPC on demand, so the explorer stays thin
//! and easy to reset.

#![allow(
    missing_debug_implementations,
    missing_docs,
    clippy::collapsible_if,
    clippy::doc_markdown,
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::option_if_let_else,
    clippy::type_complexity,
    clippy::uninlined_format_args,
    clippy::unnecessary_self_imports
)]

pub mod config;
pub mod indexer;
pub mod models;
pub mod rpc_proxy;
pub mod server;
pub mod storage;
pub mod trace;

pub use config::ExplorerConfig;
pub use server::Explorer;
