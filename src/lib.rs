//! RustNps core library.
//!
//! 这个库把 Go 版 nps 的核心边界拆成几个稳定模块：
//! - config: nps.conf / npc.conf parser
//! - protocol: nps 与 npc 的控制面协议
//! - server/client: server/client runtime
//! - relay/socks5: data-plane helpers

pub mod client;
pub mod config;
pub mod logging;
pub mod model;
pub mod mux;
pub mod protocol;
pub mod relay;
pub mod server;
pub mod socks5;
pub mod store;
pub mod tls;
pub mod web;

pub const VERSION: &str = "0.1.0";
pub const CORE_VERSION: &str = "0.1.0";
