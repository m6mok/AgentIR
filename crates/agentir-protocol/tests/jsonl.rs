use agentir_core::resources::ResourceLimits;
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
    assert_eq!(parsed["result"]["target_archive_version"], 10);
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
fn failed_version_aware_replay_does_not_publish_a_workspace() {
    let corrupted = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentir-store/tests/fixtures/corrupted-semantics-v3.json"
    );
    let mut engine = Engine::new();
    let load: Value = serde_json::from_str(
        &engine.process_line(
            &serde_json::json!({
                "command": "workspace.load",
                "request_id": "bad-semantics",
                "path": corrupted,
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(load["error"]["code"], "PERSISTENCE_FORMAT");
    let query: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"program.query","request_id":"query","workspace":"golden-v2"}"#,
    ))
    .unwrap();
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

#[test]
fn request_byte_and_depth_limits_are_checked_before_parse() {
    let line = r#"{"command":"workspace.open","request_id":"boundary","workspace":"w"}"#;
    let limits = ResourceLimits {
        jsonl_request_bytes: u64::try_from(line.len()).unwrap(),
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits.clone());
    let accepted: Value = serde_json::from_str(&engine.process_line(line)).unwrap();
    assert_eq!(accepted["ok"], true);

    let mut engine = Engine::with_limits(limits);
    let rejected: Value = serde_json::from_str(&engine.process_line(&format!("{line} "))).unwrap();
    assert_eq!(rejected["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(rejected["request_id"], "boundary");

    let depth_limits = ResourceLimits {
        json_nesting_depth: 3,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(depth_limits);
    let deep: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"workspace.open","request_id":"deep","workspace":[[[["w"]]]]}"#,
    ))
    .unwrap();
    assert_eq!(deep["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
}

#[test]
fn action_array_limit_is_structured_and_protocol_state_is_atomic() {
    let limits = ResourceLimits {
        actions_per_transaction: 1,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    let open: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"workspace.open","request_id":"open-array","workspace":"array-budget"}"#,
    ))
    .unwrap();
    assert_eq!(open["ok"], true);
    let oversized: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"spec.apply","request_id":"too-many","workspace":"array-budget","base_revision":"r0","actions":[{"kind":"create_constant","bind":"$a","type":"i32","value":1},{"kind":"create_constant","bind":"$b","type":"i32","value":2}]}"#,
    ))
    .unwrap();
    assert_eq!(oversized["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    let accepted: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"spec.apply","request_id":"one","workspace":"array-budget","base_revision":"r0","actions":[{"kind":"create_constant","bind":"$a","type":"i32","value":1}]}"#,
    ))
    .unwrap();
    assert_eq!(accepted["result"]["revision"], "r1");
    assert_eq!(accepted["result"]["bindings"]["$a"], "v1");
}

#[test]
fn invalid_utf8_unknown_fields_and_malformed_literals_are_structured() {
    let mut engine = Engine::new();
    let invalid_utf8: Value =
        serde_json::from_str(&engine.process_bytes(b"{\"request_id\":\"utf8\",\xff}")).unwrap();
    assert_eq!(invalid_utf8["request_id"], "utf8");
    assert_eq!(invalid_utf8["error"]["code"], "INVALID_REQUEST");

    for (case, expected_id) in [
        (
            r#"{"command":"workspace.open","request_id":"unknown-field","extra":1}"#,
            "unknown-field",
        ),
        (
            r#"{"command":"workspace.open","request_id":"duplicate","request_id":"again"}"#,
            "duplicate",
        ),
        (
            r#"{"command":"workspace.open","request_id":"number","workspace":1e999999}"#,
            "number",
        ),
        ("", "unknown"),
    ] {
        let response: Value = serde_json::from_str(&engine.process_line(case)).unwrap();
        assert_eq!(response["ok"], false, "{case}");
        assert_eq!(response["error"]["code"], "INVALID_REQUEST", "{case}");
        assert_eq!(response["request_id"], expected_id, "{case}");
    }
}

fn mutation_sequence(seed: u64) -> Vec<String> {
    let corpus = [
        b"{}".as_slice(),
        b"[]".as_slice(),
        b"not json".as_slice(),
        b"{\"command\":\"workspace.open\",\"request_id\":\"ok\"}".as_slice(),
        b"{\"command\":\"unknown\",\"request_id\":\"bad\"}".as_slice(),
        b"{\"command\":\"workspace.open\",\"request_id\":\"x\",\"workspace\":[]}".as_slice(),
        br#"{"command":"candidate.propose","request_id":"p","workspace":"w","candidate":"c1","base_candidate_revision":"cr1","target":"iop1","replacement":{"inputs":[{"bind":"$x","value":"iv1"}],"operations":[{"bind":"$y","opcode":"cast","operands":["$x"],"attributes":{"target_type":"i32"}}],"result":{"value":"$y"}},"expected_before_impl_hash":"hash","allow_speculative":true}"#.as_slice(),
        br#"{"command":"candidate.propose","request_id":"dup","workspace":"w","candidate":"c1","candidate":"c2","base_candidate_revision":"cr1","target":"iop1","replacement":{"inputs":[],"operations":[],"result":{"value":"$x"}},"expected_before_impl_hash":"hash"}"#.as_slice(),
    ];
    let mut state = seed;
    let mut engine = Engine::new();
    let mut results = Vec::new();
    for _ in 0..64 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let input = corpus[usize::try_from(state).unwrap() % corpus.len()];
        let response: Value = serde_json::from_str(&engine.process_bytes(input)).unwrap();
        results.push(
            response["error"]["code"]
                .as_str()
                .unwrap_or("OK")
                .to_owned(),
        );
    }
    results
}

#[test]
fn fixed_seed_protocol_mutation_classification_is_reproducible() {
    assert_eq!(mutation_sequence(7), mutation_sequence(7));
}
