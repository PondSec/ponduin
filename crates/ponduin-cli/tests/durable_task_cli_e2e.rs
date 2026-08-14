use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run(binary: &str, workspace: &Path, arguments: &[&str]) -> Output {
    Command::new(binary)
        .args(arguments)
        .current_dir(workspace)
        .env("PONDUIN_DISABLE_KEYRING", "1")
        .output()
        .expect("CLI process starts")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn durable_task_survives_real_cli_process_boundaries() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    let store = temporary.path().join("task-store");
    std::fs::create_dir(&workspace).unwrap();
    let store = store.to_string_lossy().into_owned();
    let binary = env!("CARGO_BIN_EXE_ponduin");

    let created = run(
        binary,
        &workspace,
        &[
            "task",
            "run",
            "inspect and resume this persisted coding task",
            "--store",
            &store,
            "--json",
        ],
    );
    assert!(created.status.success(), "{}", output_text(&created));
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    let task_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "running");

    let listed = run(
        binary,
        &workspace,
        &["task", "list", "--store", &store, "--json"],
    );
    assert!(listed.status.success(), "{}", output_text(&listed));
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], task_id);

    let inspected = run(
        binary,
        &workspace,
        &["task", "inspect", &task_id, "--store", &store, "--json"],
    );
    assert!(inspected.status.success(), "{}", output_text(&inspected));
    let inspected: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected["state"]["runtime"]["runtime"]["id"], task_id);
    assert!(inspected["events"].as_array().unwrap().len() >= 2);

    let resumed = run(
        binary,
        &workspace,
        &["task", "resume", &task_id, "--store", &store, "--json"],
    );
    assert!(resumed.status.success(), "{}", output_text(&resumed));
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["task"]["id"], task_id);
    assert_eq!(resumed["task"]["status"], "running");

    let events = run(
        binary,
        &workspace,
        &["task", "events", &task_id, "--store", &store],
    );
    assert!(events.status.success(), "{}", output_text(&events));
    let event_output = String::from_utf8(events.stdout).unwrap();
    let event_lines = event_output.lines().collect::<Vec<_>>();
    assert!(event_lines.len() >= 5);
    assert!(event_lines
        .iter()
        .any(|line| line.contains("checkpoint_created")));
}
