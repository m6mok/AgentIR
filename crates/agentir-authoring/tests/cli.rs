use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn run_cli(surface: &str, payload: &str) -> (std::process::ExitStatus, Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentir-authoring"))
        .args([
            "--task",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/authoring_task_two_term.json"
            ),
            "--surface",
            surface,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn authoring CLI");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write payload");
    let output = child.wait_with_output().expect("CLI output");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    let envelope = serde_json::from_str(stdout.trim()).expect("one JSON envelope");
    (
        output.status,
        envelope,
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
    )
}

#[test]
fn cli_runs_graph_incremental_and_staged_examples() {
    let cases = [
        (
            "graph",
            include_str!("../../../examples/authoring_proposal_two_term.json"),
        ),
        (
            "incremental-batch",
            include_str!("../../../examples/authoring_incremental_two_term.json"),
        ),
        (
            "staged",
            include_str!("../../../examples/authoring_staged_two_term.json"),
        ),
        (
            "auto",
            include_str!("../../../examples/authoring_staged_two_term.json"),
        ),
    ];
    for (surface, payload) in cases {
        let (status, envelope, stderr) = run_cli(surface, payload);
        assert!(
            status.success(),
            "JSON ok is authoritative; stderr={stderr}"
        );
        assert_eq!(envelope["ok"], true);
        assert_eq!(
            envelope["result"]["outputs"]["out"],
            serde_json::json!([32.0, 64.0, 96.0, 128.0])
        );
        assert_eq!(envelope["result"]["native_checked"], true);
    }
}

#[test]
fn cli_emits_one_structured_error_without_dialect_fallback() {
    let payload = r#"{"schema":"agentir.elementwise_staged.v1","stages":1,"seed":{"kind":"tensor","name":"x"},"body":[],"state":"$x"}"#;
    let (status, envelope, stderr) = run_cli("staged", payload);
    assert!(
        status.success(),
        "request envelope, not exit status, is authoritative"
    );
    assert!(stderr.is_empty());
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "SCHEMA_REJECTED");
    assert_eq!(envelope["error"]["path"], "$.body");
}
