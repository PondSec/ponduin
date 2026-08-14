use ponduin::task::{
    ActionOutcome, ActionRecord, CapabilityDiscovery, FileKind, GoalBudget, GoalEvidence,
    GoalStatus, NeedUserInput, ResourceVersion, RiskLevel, ScopedFilesystem, TaskDomain,
    TaskLimits, TaskRuntime, TaskRuntimeError, TaskStatus, ToolDisclosureRequest, ToolRegistry,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn evidence(id: &str, resources: Vec<ResourceVersion>) -> GoalEvidence {
    GoalEvidence {
        id: id.to_string(),
        kind: "end_state_verification".to_string(),
        summary: "the requested end state was observed by the runtime".to_string(),
        resources,
        revision: 0,
        valid: false,
    }
}

fn action(tool: &str, summary: &str, outcome: ActionOutcome) -> ActionRecord {
    ActionRecord {
        tool: tool.to_string(),
        summary: summary.to_string(),
        outcome,
        failure_fingerprint: None,
        process_time_ms: Some(1),
    }
}

#[test]
fn long_horizon_repair_keeps_proven_work_and_completes_only_after_verification() {
    let mut runtime = TaskRuntime::new(
        "inspect, repair, validate, and report a local project",
        None,
        TaskLimits::default(),
    )
    .unwrap();
    let root = runtime.root_goal.clone();
    let inspect = runtime
        .add_subtask(
            &root,
            "orient in the repository",
            BTreeSet::new(),
            BTreeSet::new(),
            GoalBudget::default(),
        )
        .unwrap();
    runtime.start_goal(&inspect).unwrap();
    for step in [
        "profile repository",
        "read instructions",
        "find configuration",
        "read failing source",
        "inspect the narrow validation",
        "inspect the broader validation",
    ] {
        runtime
            .record_action(
                &inspect,
                action("coding__repository_profile", step, ActionOutcome::Succeeded),
            )
            .unwrap();
    }
    runtime
        .add_evidence(&inspect, evidence("orientation", Vec::new()))
        .unwrap();
    runtime.complete_goal(&inspect).unwrap();

    let incorrect_hypothesis = runtime
        .add_subtask(
            &root,
            "change a dependency",
            [inspect.clone()].into_iter().collect(),
            BTreeSet::new(),
            GoalBudget::default(),
        )
        .unwrap();
    runtime.start_goal(&incorrect_hypothesis).unwrap();
    runtime
        .record_action(
            &incorrect_hypothesis,
            action(
                "coding__run_process",
                "dependency is not the cause",
                ActionOutcome::Failed,
            ),
        )
        .unwrap();
    let repair = runtime
        .replan(
            &incorrect_hypothesis,
            "repair the diagnosed source file",
            "the validation implicated a source-level failure instead",
        )
        .unwrap();
    runtime.start_goal(&repair).unwrap();
    for step in [
        "select the repair strategy",
        "apply the bounded repair",
        "run targeted validation",
        "run broader verification",
    ] {
        runtime
            .record_action(
                &repair,
                action("coding__run_process", step, ActionOutcome::Succeeded),
            )
            .unwrap();
    }
    runtime
        .add_evidence(&repair, evidence("validation", Vec::new()))
        .unwrap();
    runtime.complete_goal(&repair).unwrap();

    runtime
        .add_evidence(&root, evidence("reported-end-state", Vec::new()))
        .unwrap();
    runtime.complete_goal(&root).unwrap();
    runtime.complete().unwrap();

    assert_eq!(runtime.status, TaskStatus::Completed);
    assert_eq!(runtime.goals[&inspect].status, GoalStatus::Completed);
    assert_eq!(
        runtime.goals[&incorrect_hypothesis].status,
        GoalStatus::Obsolete
    );
    assert_eq!(runtime.goals[&repair].status, GoalStatus::Completed);
    assert_eq!(runtime.actions, 11);
    assert!(runtime
        .events()
        .any(|event| matches!(event.event, ponduin::task::TaskEventKind::Replanned { .. })));
}

#[test]
fn filesystem_end_state_is_scoped_and_policy_denials_are_not_successes() {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir(temporary.path().join("inbox")).unwrap();
    fs::create_dir(temporary.path().join("archive")).unwrap();
    fs::write(temporary.path().join("inbox/notes.md"), "keep this note").unwrap();
    fs::write(temporary.path().join(".env"), "SECRET=value").unwrap();
    let filesystem = ScopedFilesystem::new(temporary.path()).unwrap();

    let moved = filesystem
        .move_file("inbox/notes.md", "archive/notes.md", true)
        .unwrap();
    assert_eq!(moved.path, PathBuf::from("archive/notes.md"));
    assert_eq!(moved.kind, FileKind::Text);
    assert!(!temporary.path().join("inbox/notes.md").exists());
    assert_eq!(
        fs::read_to_string(temporary.path().join("archive/notes.md")).unwrap(),
        "keep this note"
    );
    assert!(matches!(
        filesystem.move_file(".env", "archive/environment", true),
        Err(TaskRuntimeError::PolicyDenied(_))
    ));
    assert!(matches!(
        filesystem.copy_file("archive/notes.md", "../outside.md", true),
        Err(TaskRuntimeError::Workspace(_))
    ));
}

#[test]
fn checkpoint_resume_capability_and_user_input_states_remain_authoritative() {
    let temporary = tempfile::tempdir().unwrap();
    let mut runtime = TaskRuntime::new(
        "organize ambiguous project files",
        Some(temporary.path().to_path_buf()),
        TaskLimits::default(),
    )
    .unwrap();
    let root = runtime.root_goal.clone();
    runtime
        .request_user_input(NeedUserInput {
            required_information: "archive destination".to_string(),
            reason: "two in-scope destinations would produce different results".to_string(),
            blocked_goal: root.clone(),
            allowed_options: vec!["archive".to_string(), "keep".to_string()],
        })
        .unwrap();
    assert_eq!(runtime.status, TaskStatus::Waiting);
    runtime
        .accept_user_input("the user chose archive".to_string())
        .unwrap();
    runtime.pause().unwrap();

    let checkpoint = serde_json::to_string(&runtime.checkpoint()).unwrap();
    let mut restored = TaskRuntime::restore(serde_json::from_str(&checkpoint).unwrap()).unwrap();
    restored.refresh_resources(&[]).unwrap();
    restored.resume().unwrap();
    assert_eq!(restored.status, TaskStatus::Running);

    let discovery = CapabilityDiscovery::probe(
        Some(temporary.path()),
        &["git", "ponduin-task-runtime-definitely-not-installed"],
    );
    assert!(discovery.workspace_readable);
    assert!(discovery.workspace_writable);
    assert!(discovery
        .executables
        .iter()
        .any(|capability| !capability.available));

    let tools = ToolRegistry::builtin().disclose(&ToolDisclosureRequest {
        domains: [TaskDomain::Filesystem].into_iter().collect(),
        allow_writes: false,
        allow_network: false,
        readable_workspace: true,
        writable_workspace: false,
        maximum_risk: RiskLevel::Low,
        available_capabilities: BTreeSet::new(),
    });
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["filesystem__find", "filesystem__list"]
    );
}
