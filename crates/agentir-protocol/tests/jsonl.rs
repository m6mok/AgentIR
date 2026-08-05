use agentir_protocol::Engine;
use serde_json::Value;
use std::fs;

#[test]
fn one_structured_response_is_emitted_per_line() {
    let mut engine = Engine::new();
    let open =
        engine.process_line(r#"{"command":"workspace.open","request_id":"one","workspace":"w1"}"#);
    let parsed: Value = serde_json::from_str(&open).expect("response JSON");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["request_id"], "one");

    let invalid = engine.process_line("not json");
    let parsed: Value = serde_json::from_str(&invalid).expect("error response JSON");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn saxpy_example_runs_through_protocol() {
    let mut engine = Engine::new();
    let lines = include_str!("../../../examples/saxpy.jsonl")
        .lines()
        .filter(|line| !line.is_empty());
    let responses = lines
        .map(|line| engine.process_line(line))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 5);
    for response in &responses {
        let parsed: Value = serde_json::from_str(response).expect("response JSON");
        assert_eq!(parsed["ok"], true, "{response}");
    }
    let result: Value = serde_json::from_str(&responses[4]).expect("evaluation response");
    assert_eq!(
        result["result"]["outputs"]["out"],
        serde_json::json!([12.0, 24.0, 36.0, 48.0])
    );

    let summary = engine.process_line(
        r#"{"command":"program.query","request_id":"summary","workspace":"w1","revision":"r2"}"#,
    );
    let summary: Value = serde_json::from_str(&summary).expect("summary response");
    assert_eq!(summary["result"]["semantic_canonical_version"], 1);
    assert!(summary["result"]["spec_hash"].as_str().is_some());

    let semantic = engine.process_line(
        r#"{"command":"program.query","request_id":"semantic","workspace":"w1","revision":"r2","view":"semantic_canonical"}"#,
    );
    let semantic: Value = serde_json::from_str(&semantic).expect("semantic response");
    assert_eq!(semantic["ok"], true, "{semantic}");
    assert_eq!(semantic["result"]["semantic_canonical_version"], 1);
    assert!(
        semantic["result"]["canonical_byte_length"]
            .as_u64()
            .is_some_and(|length| length > 0)
    );
}

#[test]
fn workspace_archive_commands_round_trip_across_engines() {
    let path = std::env::temp_dir().join(format!("agentir-protocol-{}.json", std::process::id()));
    let _ignored = fs::remove_file(&path);
    let mut writer = Engine::new();
    for request in [
        serde_json::json!({
            "command": "workspace.open",
            "request_id": "open",
            "workspace": "persisted",
        }),
        serde_json::json!({
            "command": "spec.apply",
            "request_id": "build",
            "workspace": "persisted",
            "base_revision": "r0",
            "actions": [
                {"kind":"create_parameter","bind":"$x","name":"x","type":"f32"},
                {"kind":"set_output","name":"out","value":"$x"},
            ],
        }),
        serde_json::json!({
            "command": "workspace.save",
            "request_id": "save",
            "workspace": "persisted",
            "path": path,
        }),
    ] {
        let response = writer.process_line(&request.to_string());
        let parsed: Value = serde_json::from_str(&response).expect("response");
        assert_eq!(parsed["ok"], true, "{response}");
    }

    let mut reader = Engine::new();
    let load = reader.process_line(
        &serde_json::json!({
            "command": "workspace.load",
            "request_id": "load",
            "path": path,
        })
        .to_string(),
    );
    let loaded: Value = serde_json::from_str(&load).expect("load response");
    assert_eq!(loaded["ok"], true, "{load}");
    assert_eq!(loaded["result"]["replay"]["revisions_verified"], 2);

    let query = reader.process_line(
        r#"{"command":"program.query","request_id":"query","workspace":"persisted","revision":"r1"}"#,
    );
    let queried: Value = serde_json::from_str(&query).expect("query response");
    assert_eq!(queried["ok"], true, "{query}");
    assert_eq!(queried["result"]["revision"], "r1");
    fs::remove_file(&path).expect("cleanup archive");
}

#[test]
fn migrate_archive_command_is_atomic_and_reports_versions() {
    let source = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentir-store/tests/fixtures/minimal-v1.json"
    );
    let destination = std::env::temp_dir().join(format!(
        "agentir-protocol-migrated-{}.json",
        std::process::id()
    ));
    let _ignored = fs::remove_file(&destination);
    let mut engine = Engine::new();
    let response = engine.process_line(
        &serde_json::json!({
            "command": "workspace.migrate_archive",
            "request_id": "m1",
            "source_path": source,
            "destination_path": destination,
            "overwrite": false,
        })
        .to_string(),
    );
    let parsed: Value = serde_json::from_str(&response).expect("migration response");
    assert_eq!(parsed["ok"], true, "{response}");
    assert_eq!(parsed["result"]["source_archive_version"], 1);
    assert_eq!(parsed["result"]["target_archive_version"], 2);
    assert!(parsed["result"]["new_archive_hash"].as_str().is_some());

    let existing = engine.process_line(
        &serde_json::json!({
            "command": "workspace.migrate_archive",
            "request_id": "m2",
            "source_path": source,
            "destination_path": destination,
            "overwrite": false,
        })
        .to_string(),
    );
    let parsed: Value = serde_json::from_str(&existing).expect("existing response");
    assert_eq!(parsed["ok"], false, "{existing}");
    assert_eq!(parsed["error"]["code"], "PERSISTENCE_IO");
    fs::remove_file(destination).expect("cleanup migrated archive");
}

#[test]
fn failed_archive_load_does_not_publish_a_workspace() {
    let corrupted = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentir-store/tests/fixtures/corrupted-v1.json"
    );
    let mut engine = Engine::new();
    let load = engine.process_line(
        &serde_json::json!({
            "command": "workspace.load",
            "request_id": "bad-load",
            "path": corrupted,
        })
        .to_string(),
    );
    let load: Value = serde_json::from_str(&load).expect("load response");
    assert_eq!(load["error"]["code"], "PERSISTENCE_INTEGRITY");

    let query = engine.process_line(
        r#"{"command":"program.query","request_id":"query","workspace":"corrupted"}"#,
    );
    let query: Value = serde_json::from_str(&query).expect("query response");
    assert_eq!(query["error"]["code"], "WORKSPACE_NOT_FOUND");
}

#[test]
fn semantic_query_rejects_unfrozen_specs_structurally() {
    let mut engine = Engine::new();
    for request in [
        r#"{"command":"workspace.open","request_id":"open","workspace":"draft"}"#,
        r#"{"command":"spec.apply","request_id":"build","workspace":"draft","base_revision":"r0","actions":[{"kind":"create_parameter","bind":"$x","name":"x","type":"f32"},{"kind":"set_output","name":"out","value":"$x"}]}"#,
    ] {
        let response = engine.process_line(request);
        assert!(response.contains("\"ok\":true"), "{response}");
    }
    let query = engine.process_line(
        r#"{"command":"program.query","request_id":"semantic","workspace":"draft","view":"semantic_canonical"}"#,
    );
    let query: Value = serde_json::from_str(&query).expect("query response");
    assert_eq!(query["error"]["code"], "SPEC_NOT_COMPLETE");
}
