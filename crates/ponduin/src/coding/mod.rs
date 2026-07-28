//! Internal, provider-independent coding-agent services.
//!
//! These modules are part of the agent core. They deliberately do not depend
//! on MCP or platform-extension dispatch.

pub mod config;
pub mod strategy;
pub mod workspace;

pub use config::CodingConfig;
pub use strategy::CodingTaskMode;
pub use workspace::CodingWorkspace;
