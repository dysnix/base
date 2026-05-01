#![doc = include_str!("../README.md")]

mod config;
pub use config::ExplorerConfig;

mod indexer;
pub use indexer::Indexer;

mod models;
pub use models::{
    ActivityItem, AddrLabel, AddressDetail, BlockDetail, BlockListItem, Erc20TransferDetail,
    LogDetail, PageCtx, StatsBlock, TxBlockMeta, TxDetail, TxListItem,
};

mod rpc_proxy;
pub use rpc_proxy::{BaseBlock, BaseReceipt, BaseTransaction, RpcClient};

mod server;
pub use server::{AppState, Explorer};

mod storage;
pub use storage::{
    ActivityRole, ActivityRow, ActivityWrite, BlockRow, BlockWrite, Stats, Storage, TxRow,
};

mod trace;
pub use trace::TraceNode;
