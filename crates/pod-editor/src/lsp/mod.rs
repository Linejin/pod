//! LSP client: own JSON-RPC over stdio on top of `lsp-types`.
//!
//! - [`rpc`]: base-protocol framing and message classification;
//! - [`client`]: request/response matching over any stream pair;
//! - [`job`]: Windows Job Object owning a server's process tree;
//! - [`registry`]: languages, server configs, root detection, PATH lookup;
//! - [`server`]: spawn + `initialize` + shutdown of one process;
//! - [`store`]: the per-worktree GPUI entity the editor talks to;
//! - [`snippet`]: snippet text + tab stops, and the tab-stop session;
//! - [`providers`]: adapters to gpui-component's editor provider traits.

pub mod client;
pub mod job;
pub mod providers;
pub mod registry;
pub mod rpc;
pub mod server;
pub mod snippet;
pub mod store;

/// The `lsp-types` version the store speaks (hosts and tests build items with it).
pub use lsp_types as types;
pub use registry::{Language, ServerConfig, ServerRegistry, language_for_path};
pub use store::{
    CompletionExtra, FileEdit, Location, LspStore, LspStoreEvent, RunningServer, ServerKey,
    ServerState, ServerStatus,
};
