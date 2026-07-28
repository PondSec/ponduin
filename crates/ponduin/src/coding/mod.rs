//! Internal, provider-independent coding-agent services.
//!
//! These modules are part of the agent core. They deliberately do not depend
//! on MCP or platform-extension dispatch.

pub mod agent;
pub mod config;
pub mod file;
pub mod git;
pub mod instructions;
pub mod intelligence;
pub mod patch;
pub mod process;
pub mod repository;
pub mod search;
pub mod sensitive;
pub mod strategy;
pub mod tools;
pub mod workspace;

pub use agent::CodingAgent;
pub use config::CodingConfig;
pub use file::FileSnapshot;
pub use git::GitRepository;
pub use instructions::RepositoryInstructions;
pub use intelligence::RepositoryIntelligence;
pub use patch::PatchEngine;
pub use process::ProcessRunner;
pub use repository::RepositoryProfile;
pub use search::RepositorySearch;
pub use strategy::CodingTaskMode;
pub use workspace::CodingWorkspace;
