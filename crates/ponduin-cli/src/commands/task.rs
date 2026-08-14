use anyhow::Result;
use clap::Subcommand;
use ponduin::task::durable::TaskStore;
use ponduin::task::TaskLimits;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum TaskCommand {
    /// Create and durably start a task in the selected workspace
    Run {
        #[arg(help = "The user goal for this task")]
        goal: String,
        #[arg(long, value_name = "PATH", help = "Workspace owned by the task")]
        workspace: Option<PathBuf>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
        #[arg(long, help = "Emit structured JSON")]
        json: bool,
    },
    /// List durable tasks in a local task store
    List {
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
        #[arg(long, help = "Emit structured JSON")]
        json: bool,
    },
    /// Show a task's persisted state and execution journal
    Inspect {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
        #[arg(long, help = "Emit structured JSON")]
        json: bool,
    },
    /// Reload, recover, validate the workspace, and continue a task
    Resume {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
        #[arg(long, help = "Emit structured JSON")]
        json: bool,
    },
    /// Persist user input for a task that is waiting for a safe decision
    Input {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(help = "Concise user instruction")]
        answer: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
    },
    /// Persist additional user guidance and replan only named goals
    Steer {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(help = "Concise user constraint")]
        guidance: String,
        #[arg(
            long = "goal",
            value_name = "GOAL_ID",
            required = true,
            action = clap::ArgAction::Append,
            help = "Affected goal identifier; may be specified more than once"
        )]
        goals: Vec<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
    },
    /// Persist a cancellation decision
    Cancel {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(help = "Reason for cancelling the task")]
        reason: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
    },
    /// Print the append-only execution journal
    Events {
        #[arg(help = "Task identifier")]
        task_id: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Directory that stores durable task state"
        )]
        store: Option<PathBuf>,
    },
}

pub fn handle_task_command(command: TaskCommand) -> Result<()> {
    match command {
        TaskCommand::Run {
            goal,
            workspace,
            store,
            json,
        } => {
            let workspace = resolve_workspace(workspace)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let task = store.create_task(goal, Some(workspace), TaskLimits::default())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&task.summary())?);
            } else {
                println!(
                    "Created durable task {} ({:?}). Resume it with `ponduin task resume {} --store {}`.",
                    task.id().as_str(),
                    task.runtime().status,
                    task.id().as_str(),
                    store.root().display()
                );
            }
            Ok(())
        }
        TaskCommand::List { store, json } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let tasks = store.list()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else if tasks.is_empty() {
                println!("No durable tasks in {}.", store.root().display());
            } else {
                for task in tasks {
                    println!(
                        "{}\t{:?}\tactions={}\trevision={}\t{}",
                        task.id.as_str(),
                        task.status,
                        task.actions,
                        task.revision,
                        task.original_goal
                    );
                }
            }
            Ok(())
        }
        TaskCommand::Inspect {
            task_id,
            store,
            json,
        } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let task = store.load(&task_id)?;
            let events = task.events()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "state": task.state(),
                        "events": events,
                    }))?
                );
            } else {
                let summary = task.summary();
                println!(
                    "{}\nstatus: {:?}\nactions: {}\nrevision: {}\nevents: {}\ntool calls: {}",
                    summary.id.as_str(),
                    summary.status,
                    summary.actions,
                    summary.revision,
                    events.len(),
                    task.state().tool_calls.len(),
                );
            }
            Ok(())
        }
        TaskCommand::Resume {
            task_id,
            store,
            json,
        } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let mut task = store.load(&task_id)?;
            let recovery = task.resume()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "task": task.summary(),
                        "recovery": {
                            "retry_scheduled": recovery.retry_scheduled,
                            "requires_user_input": recovery.requires_user_input,
                            "invalidated_evidence": recovery.invalidated_evidence,
                            "replanned_goals": recovery
                                .replanned_goals
                                .iter()
                                .map(|goal| goal.as_str())
                                .collect::<Vec<_>>(),
                        },
                    }))?
                );
            } else {
                println!(
                    "{} resumed as {:?}; invalidated evidence: {}, replanned goals: {}, recovery decisions: {}.",
                    task.id().as_str(),
                    task.runtime().status,
                    recovery.invalidated_evidence,
                    recovery.replanned_goals.len(),
                    recovery.retry_scheduled.len() + recovery.requires_user_input.len(),
                );
            }
            Ok(())
        }
        TaskCommand::Input {
            task_id,
            answer,
            store,
        } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let mut task = store.load(&task_id)?;
            task.provide_user_input(answer)?;
            println!("{} accepted user input.", task.id().as_str());
            Ok(())
        }
        TaskCommand::Steer {
            task_id,
            guidance,
            goals,
            store,
        } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let mut task = store.load(&task_id)?;
            let affected_goals = goals
                .iter()
                .map(|goal_id| {
                    task.runtime()
                        .goals
                        .values()
                        .find(|goal| goal.id.as_str() == goal_id)
                        .map(|goal| goal.id.clone())
                        .ok_or_else(|| anyhow::anyhow!("unknown task goal: {goal_id}"))
                })
                .collect::<Result<Vec<_>>>()?;
            let replacements = task.steer(guidance, &affected_goals)?;
            println!(
                "{} persisted user steering and replanned {} goal(s).",
                task.id().as_str(),
                replacements.len()
            );
            Ok(())
        }
        TaskCommand::Cancel {
            task_id,
            reason,
            store,
        } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let mut task = store.load(&task_id)?;
            task.cancel(reason)?;
            println!("{} cancelled.", task.id().as_str());
            Ok(())
        }
        TaskCommand::Events { task_id, store } => {
            let workspace = resolve_workspace(None)?;
            let store = TaskStore::new(resolve_store(store, &workspace))?;
            let task = store.load(&task_id)?;
            for event in task.events()? {
                println!("{}", serde_json::to_string(&event)?);
            }
            Ok(())
        }
    }
}

fn resolve_workspace(workspace: Option<PathBuf>) -> Result<PathBuf> {
    Ok(workspace.unwrap_or(std::env::current_dir()?))
}

fn resolve_store(store: Option<PathBuf>, workspace: &std::path::Path) -> PathBuf {
    store.unwrap_or_else(|| workspace.join(".ponduin/tasks"))
}
