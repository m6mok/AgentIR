use agentir_core::{
    backend::{artifact_hash, device_fingerprint_hash, measurement_hash},
    ids::{ArtifactId, MeasurementId},
};
use agentir_protocol::Engine;
use agentir_store::load_workspace_bytes;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

fn run_example() -> Vec<Value> {
    let mut engine = Engine::new();
    include_str!("../../../examples/backend_saxpy_wgsl.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let response = engine.process_line(line);
            let parsed: Value = serde_json::from_str(&response).expect("response JSON");
            assert_eq!(parsed["ok"], true, "{response}");
            parsed
        })
        .collect()
}

#[test]
fn stage5_wgsl_artifact_is_offline_validated_and_reference_exact() {
    let responses = run_example();
    let lowered = responses
        .iter()
        .find(|response| response["request_id"] == "backend-lower")
        .unwrap();
    let artifact = responses
        .iter()
        .find(|response| response["request_id"] == "artifact-emit")
        .unwrap();
    let evaluated = responses
        .iter()
        .find(|response| response["request_id"] == "artifact-reference")
        .unwrap();
    assert_eq!(lowered["result"]["equivalent_to_schedule"], true);
    assert_eq!(artifact["result"]["offline_valid"], true);
    assert_ne!(
        lowered["result"]["query"]["backend_hash"],
        artifact["result"]["query"]["artifact_hash"]
    );
    assert_eq!(
        evaluated["result"]["evaluation"]["evaluation"]["outputs"]["out"],
        json!([12.0, 24.0, 36.0, 48.0])
    );
}

#[test]
fn stage5_emission_is_byte_and_hash_deterministic() {
    let first = run_example();
    let second = run_example();
    let select = |responses: &[Value], request_id: &str| {
        responses
            .iter()
            .find(|response| response["request_id"] == request_id)
            .unwrap()["result"]
            .clone()
    };
    assert_eq!(
        select(&first, "backend-lower"),
        select(&second, "backend-lower")
    );
    assert_eq!(
        select(&first, "artifact-emit"),
        select(&second, "artifact-emit")
    );
}

#[test]
fn protocol_never_accepts_agent_supplied_wgsl_or_backend_nodes() {
    let mut engine = Engine::new();
    let response = engine.process_line(
        r#"{"command":"artifact.emit","request_id":"bad","workspace":"w1","backend_plan":"bp1","backend_revision":"br1","expected_backend_hash":"00","wgsl":"@compute fn injected() {}"}"#,
    );
    let parsed: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn stage5_schedule_variants_emit_offline_valid_artifacts() {
    for source in [
        include_str!("../../../examples/backend_serial.jsonl"),
        include_str!("../../../examples/backend_tiled.jsonl"),
        include_str!("../../../examples/backend_remainder.jsonl"),
        include_str!("../../../examples/backend_fused.jsonl"),
        include_str!("../../../examples/backend_vectorized.jsonl"),
        include_str!("../../../examples/backend_reuse.jsonl"),
        include_str!("../../../examples/backend_guarded_memory.jsonl"),
        include_str!("../../../examples/equality_to_artifact.jsonl"),
    ] {
        let mut engine = Engine::new();
        for line in source.lines().filter(|line| !line.is_empty()) {
            let response = engine.process_line(line);
            let parsed: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["ok"], true, "{response}");
        }
    }
}

#[test]
fn vector_unroll_and_guard_branches_remain_explicit_and_reference_exact() {
    let mut vector_engine = Engine::new();
    let vector_responses = include_str!("../../../examples/backend_vectorized.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(&vector_engine.process_line(line)).unwrap())
        .collect::<Vec<_>>();
    let lowered = vector_responses
        .iter()
        .find(|response| response["request_id"] == "backend")
        .unwrap();
    assert_eq!(lowered["result"]["query"]["vector_widths"], json!([4]));
    assert_eq!(lowered["result"]["query"]["unroll_factors"], json!([2]));

    let mut guarded_engine = Engine::new();
    for line in include_str!("../../../examples/backend_guarded_memory.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
    {
        let response = guarded_engine.process_line(line);
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
    }
    let evaluate = |engine: &mut Engine, outcome: bool| {
        let response = engine.process_line(
            &json!({
                "command": "artifact.reference_evaluate",
                "request_id": format!("guard-{outcome}"),
                "workspace": "w1",
                "artifact": "art1",
                "inputs": {"x": [1.0, 2.0, 3.0, 4.0]},
                "guard_outcomes": {"mg1": outcome},
            })
            .to_string(),
        );
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["ok"], true, "{response}");
        parsed
    };
    let true_path = evaluate(&mut guarded_engine, true);
    let false_path = evaluate(&mut guarded_engine, false);
    assert_eq!(
        true_path["result"]["evaluation"]["evaluation"]["outputs"],
        false_path["result"]["evaluation"]["evaluation"]["outputs"]
    );
    assert_eq!(true_path["result"]["trace"]["guard_branch"], true);
    assert_eq!(false_path["result"]["trace"]["guard_branch"], false);
    for response in [&true_path, &false_path] {
        assert!(
            response["result"]["trace"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "binding")
        );
    }
}

#[test]
fn stage5_hash_domains_are_independent_from_runtime_observations() {
    let loaded = load_workspace_bytes(include_bytes!(
        "../../agentir-store/tests/fixtures/measurement-record-v9.json"
    ))
    .expect("measurement fixture");
    let package = loaded
        .workspace
        .artifact_package(&ArtifactId::new("art1"))
        .expect("artifact");
    assert_eq!(artifact_hash(package).unwrap(), package.artifact_hash);
    let record = loaded
        .workspace
        .measurement_query(&MeasurementId::new("meas1"))
        .expect("measurement");
    assert_eq!(measurement_hash(record).unwrap(), record.measurement_hash);
    assert_eq!(
        device_fingerprint_hash(&record.device).unwrap(),
        record.device_fingerprint_hash
    );
    assert_ne!(
        package.artifact_hash.to_string(),
        record.measurement_hash.to_string()
    );
    assert_ne!(
        package.artifact_hash.to_string(),
        record.device_fingerprint_hash.to_string()
    );
    let mut changed = package.clone();
    changed.modules[0].wgsl.push_str("// exact byte change\n");
    assert_ne!(artifact_hash(&changed).unwrap(), package.artifact_hash);
}

#[test]
fn deterministic_device_unavailable_path_does_not_mutate_artifact_state() {
    let mut engine = Engine::new();
    for line in include_str!("../../../examples/backend_saxpy_wgsl.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
    {
        let response = engine.process_line(line);
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
    }
    let before = engine.process_line(
        r#"{"command":"artifact.query","request_id":"before","workspace":"w1","artifact":"art1"}"#,
    );
    let unavailable = engine.process_line(
        r#"{"command":"artifact.execute","request_id":"execute","workspace":"w1","artifact":"art1","expected_artifact_hash":"dda87c5a34513cabcb501cd94f573deff55f7f56afa55664a78d36b88f777300","adapter":4294967295,"inputs":{"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}}"#,
    );
    let parsed: Value = serde_json::from_str(&unavailable).unwrap();
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"]["code"], "DEVICE_UNAVAILABLE");
    let after = engine.process_line(
        r#"{"command":"artifact.query","request_id":"after","workspace":"w1","artifact":"art1"}"#,
    );
    let before: Value = serde_json::from_str(&before).unwrap();
    let after: Value = serde_json::from_str(&after).unwrap();
    assert_eq!(before["result"], after["result"]);
}

#[test]
fn reduce_lowering_is_a_structured_atomic_rejection() {
    let mut engine = Engine::new();
    let responses = include_str!("../../../examples/backend_rejected_reduce.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(&engine.process_line(line)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.last().unwrap()["ok"], false);
    assert_eq!(
        responses.last().unwrap()["error"]["code"],
        "UNSUPPORTED_BACKEND_LOWERING"
    );
}

#[test]
fn archive_v9_replays_backend_ids_hashes_and_exact_wgsl_package() {
    let mut engine = Engine::new();
    for line in include_str!("../../../examples/backend_saxpy_wgsl.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
    {
        let response = engine.process_line(line);
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("agentir-stage5-{nonce}.json"));
    let save = engine.process_line(
        &json!({
            "command": "workspace.save",
            "request_id": "save",
            "workspace": "w1",
            "path": path,
        })
        .to_string(),
    );
    assert_eq!(serde_json::from_str::<Value>(&save).unwrap()["ok"], true);
    let mut restored = Engine::new();
    let load = restored.process_line(
        &json!({
            "command": "workspace.load",
            "request_id": "load",
            "path": path,
        })
        .to_string(),
    );
    assert_eq!(
        serde_json::from_str::<Value>(&load).unwrap()["ok"],
        true,
        "{load}"
    );
    let query = restored.process_line(
        r#"{"command":"artifact.query","request_id":"query","workspace":"w1","artifact":"art1"}"#,
    );
    let parsed: Value = serde_json::from_str(&query).unwrap();
    assert_eq!(parsed["ok"], true, "{query}");
    assert_eq!(
        parsed["result"]["artifact_hash"],
        "dda87c5a34513cabcb501cd94f573deff55f7f56afa55664a78d36b88f777300"
    );
    let _ = std::fs::remove_file(path);
}
