//! The public MCP bridge: a coding agent talks MCP over stdio to `visualyze mcp`,
//! which reads its files from the project, checks them with the public contract,
//! and drives the local viewer.
mod bridge;
pub mod protocol;

pub use bridge::{Bridge, run};
