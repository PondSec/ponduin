//! Internal, provider-independent coding-agent services.
//!
//! These modules are part of the agent core. They deliberately do not depend
//! on MCP or platform-extension dispatch.

pub mod agent;
pub mod capabilities;
pub mod config;
pub mod context;
pub mod diagnostic;
pub mod embedding;
pub mod file;
pub mod git;
pub mod instructions;
pub mod intelligence;
pub mod lsp;
pub mod outcome;
pub mod patch;
pub mod process;
pub mod project;
pub mod repository;
pub mod review;
pub mod search;
pub mod sensitive;
pub mod strategy;
pub mod tools;
pub mod validation;
pub mod workflow;
pub mod workspace;

pub use agent::CodingAgent;
pub use capabilities::{
    CapabilitySupport, CodingSuitability, ModelCapabilityProfile, PerformanceClass, ResourceClass,
};
pub use config::CodingConfig;
pub use context::ContextPlanner;
pub use diagnostic::DiagnosticAnalyzer;
pub use embedding::LocalEmbeddingIndex;
pub use file::FileSnapshot;
pub use git::GitRepository;
pub use instructions::RepositoryInstructions;
pub use intelligence::RepositoryIntelligence;
pub use lsp::LanguageServerClient;
pub use outcome::{ActionFailureKind, ActionResult, RecoveryDecision};
pub use patch::PatchEngine;
pub use process::ProcessRunner;
pub use project::ProjectDiscovery;
pub use repository::RepositoryProfile;
pub use review::ReviewAnalyzer;
pub use search::RepositorySearch;
pub use strategy::MODEL_ROUTING_GUIDANCE;
pub use validation::ValidationService;
pub use workflow::{CodingWorkflow, TaskInteractionMode, WorkflowTaskState};
pub use workspace::CodingWorkspace;
