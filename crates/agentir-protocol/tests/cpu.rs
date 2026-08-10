use agentir_core::{
    cpu::{
        CpuArtifactPackage, CpuInstruction, CpuScalarOpcode, cpu_artifact_hash, verify_cpu_artifact,
    },
    ids::{CpuArtifactId, WorkspaceId},
    resources::ResourceLimits,
};
use agentir_protocol::Engine;
use agentir_store::{load_workspace, load_workspace_bytes};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn reseal(package: &mut CpuArtifactPackage) {
    package.cpu_artifact_hash = cpu_artifact_hash(package).unwrap();
    package.certificate.cpu_artifact_hash = package.cpu_artifact_hash.clone();
    package.id = CpuArtifactId::new(format!(
        "cpuart-{}",
        &package.cpu_artifact_hash.as_str()[..16]
    ));
}

fn run(source: &str) -> (Engine, Vec<Value>) {
    let mut engine = Engine::new();
    let responses = source
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let response = engine.process_line(line);
            let parsed: Value = serde_json::from_str(&response).expect("response JSON");
            assert_eq!(parsed["ok"], true, "{response}");
            parsed
        })
        .collect();
    (engine, responses)
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("agentir-stage8a-{label}-{nonce}.json"))
}

fn save_engine(engine: &mut Engine, label: &str) -> PathBuf {
    let path = temp_path(label);
    let response = engine.process_line(
        &json!({
            "command": "workspace.save",
            "request_id": "save",
            "workspace": "w1",
            "path": path,
        })
        .to_string(),
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["ok"],
        true,
        "{response}"
    );
    path
}

#[test]
fn cpu_saxpy_is_exact_and_matches_the_reference_evaluator() {
    let (_, responses) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let reference = responses
        .iter()
        .find(|response| response["request_id"] == "reference")
        .unwrap();
    let execution = responses
        .iter()
        .find(|response| response["request_id"] == "execute")
        .unwrap();
    assert_eq!(
        execution["result"]["outputs"]["out"],
        json!([12.0, 24.0, 36.0, 48.0])
    );
    assert_eq!(
        execution["result"]["outputs"],
        reference["result"]["outputs"]
    );
    assert_eq!(execution["result"]["observation"]["elements_executed"], 4);
}

#[test]
fn cpu_packages_are_byte_and_hash_deterministic() {
    let (mut first, first_responses) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let (mut second, second_responses) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let emitted = |responses: &[Value]| {
        responses
            .iter()
            .find(|response| response["request_id"] == "emit")
            .unwrap()["result"]
            .clone()
    };
    assert_eq!(emitted(&first_responses), emitted(&second_responses));
    let first_path = save_engine(&mut first, "determinism-a");
    let second_path = save_engine(&mut second, "determinism-b");
    let first_loaded = load_workspace(&first_path).unwrap();
    let second_loaded = load_workspace(&second_path).unwrap();
    let id = CpuArtifactId::new("cpuart-c6eb17c4671f1cb8");
    let first_package = first_loaded.workspace.cpu_artifact_package(&id).unwrap();
    let second_package = second_loaded.workspace.cpu_artifact_package(&id).unwrap();
    assert_eq!(
        serde_json::to_vec(first_package).unwrap(),
        serde_json::to_vec(second_package).unwrap()
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
}

#[test]
fn constants_opcodes_and_ordered_operands_change_cpu_artifact_identity() {
    let (mut engine, _) = run(include_str!(
        "../../../examples/cpu_scalar_elementwise.jsonl"
    ));
    let path = save_engine(&mut engine, "identity");
    let loaded = load_workspace(&path).unwrap();
    let package = loaded
        .workspace
        .cpu_artifact_package(&CpuArtifactId::new("cpuart-c6572dc6ab61af87"))
        .unwrap();
    let original = cpu_artifact_hash(package).unwrap();

    let mut constant = package.clone();
    constant.constants[0].bits = "0x40400000".to_owned();
    assert_ne!(cpu_artifact_hash(&constant).unwrap(), original);

    let mut opcode = package.clone();
    let CpuInstruction::MapF32 { body, .. } = &mut opcode.functions[0].instructions[1] else {
        panic!("map instruction")
    };
    body.instructions[0].opcode = CpuScalarOpcode::AddF32;
    assert_ne!(cpu_artifact_hash(&opcode).unwrap(), original);

    let mut operands = package.clone();
    let CpuInstruction::MapF32 { body, .. } = &mut operands.functions[0].instructions[1] else {
        panic!("map instruction")
    };
    body.instructions[0].operands.reverse();
    assert_ne!(cpu_artifact_hash(&operands).unwrap(), original);
    let _ = std::fs::remove_file(path);
}

#[test]
fn structurally_resealed_noncanonical_packages_are_rejected() {
    let (mut engine, _) = run(include_str!(
        "../../../examples/cpu_scalar_elementwise.jsonl"
    ));
    let path = save_engine(&mut engine, "structural-rejection");
    let loaded = load_workspace(&path).unwrap();
    let package = loaded
        .workspace
        .cpu_artifact_package(&CpuArtifactId::new("cpuart-c6572dc6ab61af87"))
        .unwrap();

    let mut uppercase_constant = package.clone();
    uppercase_constant.constants[0].bits = "0x3F800000".to_owned();
    reseal(&mut uppercase_constant);
    assert!(verify_cpu_artifact(&uppercase_constant).is_err());

    let mut invented_certificate = package.clone();
    invented_certificate
        .certificate
        .conditions
        .push("client_invented_condition".to_owned());
    reseal(&mut invented_certificate);
    assert!(verify_cpu_artifact(&invented_certificate).is_err());

    let mut incompatible_extent = package.clone();
    let CpuInstruction::MapF32 { extent, .. } =
        &mut incompatible_extent.functions[0].instructions[1]
    else {
        panic!("map instruction")
    };
    *extent = agentir_core::cpu::CpuExtent::Static { value: 4 };
    reseal(&mut incompatible_extent);
    assert!(verify_cpu_artifact(&incompatible_extent).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn execution_limits_cover_intermediate_storage_and_instruction_work() {
    let (mut engine, _) = run(include_str!(
        "../../../examples/cpu_scalar_elementwise.jsonl"
    ));
    let path = save_engine(&mut engine, "projected-work");
    let loaded = load_workspace(&path).unwrap();
    let package = loaded
        .workspace
        .cpu_artifact_package(&CpuArtifactId::new("cpuart-c6572dc6ab61af87"))
        .unwrap();
    let limits = ResourceLimits {
        execution_elements: 7,
        ..ResourceLimits::default()
    };
    let error = agentir_backend_cpu::execute(
        package,
        &BTreeMap::from([("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0]))]),
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.code, agentir_core::ErrorCode::ResourceLimitExceeded);
    let _ = std::fs::remove_file(path);
}

#[test]
fn compiler_ids_runtime_policy_and_counters_do_not_change_cpu_artifact_hash() {
    let (mut engine, _) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let path = save_engine(&mut engine, "policy");
    let loaded = load_workspace(&path).unwrap();
    let package = loaded
        .workspace
        .cpu_artifact_package(&CpuArtifactId::new("cpuart-c6eb17c4671f1cb8"))
        .unwrap();
    let original = cpu_artifact_hash(package).unwrap();
    let mut changed_id = package.clone();
    changed_id.id = CpuArtifactId::new("cpuart-untrusted-id");
    assert_eq!(cpu_artifact_hash(&changed_id).unwrap(), original);
    let bytecode = serde_json::to_string(&package.functions).unwrap();
    assert!(!bytecode.contains("iop"));
    assert!(!bytecode.contains("iv"));

    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
        ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
    ]);
    let default =
        agentir_backend_cpu::execute(package, &inputs, &ResourceLimits::default()).unwrap();
    let changed_limits = ResourceLimits {
        execution_elements: 10_000,
        execution_bytes: 10_000,
        ..ResourceLimits::default()
    };
    let changed = agentir_backend_cpu::execute(package, &inputs, &changed_limits).unwrap();
    assert_eq!(default.outputs, changed.outputs);
    assert_eq!(default.cpu_artifact_hash, original);
    assert_eq!(changed.cpu_artifact_hash, original);
    let _ = std::fs::remove_file(path);
}

#[test]
fn clients_cannot_supply_bytecode_or_correctness_claims() {
    let mut engine = Engine::new();
    for field in [
        r#""bytecode":[]"#,
        r#""bindings":[]"#,
        r#""equivalent_to_schedule":true"#,
        r#""certificate":{}"#,
    ] {
        let request = format!(
            r#"{{"command":"cpu_artifact.emit","request_id":"bad","workspace":"w1","schedule_plan":"sp1","schedule_revision":"sr1","expected_schedule_hash":"00",{field}}}"#
        );
        let parsed: Value = serde_json::from_str(&engine.process_line(&request)).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "INVALID_REQUEST");
    }
}

#[test]
fn unsupported_lowering_and_failed_execution_are_atomic() {
    let mut reduction = Engine::new();
    let responses = include_str!("../../../examples/cpu_rejected_reduction.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(&reduction.process_line(line)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        responses.last().unwrap()["error"]["code"],
        "UNSUPPORTED_CPU_LOWERING"
    );
    let list: Value =
        serde_json::from_str(&reduction.process_line(
            r#"{"command":"cpu_artifact.list","request_id":"list","workspace":"w1"}"#,
        ))
        .unwrap();
    assert_eq!(list["result"], json!([]));

    let (mut engine, _) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let before: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"cpu_artifact.query","request_id":"before","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8"}"#,
    )).unwrap();
    let failed: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"cpu_artifact.execute","request_id":"bad-input","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8","expected_cpu_artifact_hash":"c6eb17c4671f1cb8988e92b275357d80a921da61d423bc12211117fef7ea9025","inputs":{"a":2.0,"x":[1.0,2.0],"y":[10.0,20.0,30.0]}}"#,
    )).unwrap();
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["error"]["code"], "CPU_EXECUTION_INPUT_MISMATCH");
    let after: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"cpu_artifact.query","request_id":"after","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8"}"#,
    )).unwrap();
    assert_eq!(before["result"], after["result"]);
}

#[test]
fn archive_v11_round_trip_verifies_cpu_packages_without_execution() {
    let (mut engine, _) = run(include_str!("../../../examples/cpu_saxpy.jsonl"));
    let path = save_engine(&mut engine, "archive");
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("instructions_executed"));
    let loaded = load_workspace_bytes(&bytes).unwrap();
    assert_eq!(loaded.metadata.format_version, 11);
    assert_eq!(loaded.replay.cpu_artifacts_verified, 1);
    assert_eq!(loaded.replay.cpu_artifact_events_replayed, 1);
    assert_eq!(loaded.workspace.id(), &WorkspaceId::new("w1"));
    loaded
        .workspace
        .cpu_artifact_check(&CpuArtifactId::new("cpuart-c6eb17c4671f1cb8"))
        .unwrap();

    let mut stale_cursor = loaded.workspace.snapshot();
    stale_cursor.cpu_artifact_store.events[0]
        .event
        .schedule_event_cursor = 0;
    assert!(agentir_core::Workspace::from_snapshot(stale_cursor).is_err());

    let mut duplicate_event = loaded.workspace.snapshot();
    duplicate_event
        .cpu_artifact_store
        .events
        .push(duplicate_event.cpu_artifact_store.events[0].clone());
    assert!(agentir_core::Workspace::from_snapshot(duplicate_event).is_err());
    let _ = std::fs::remove_file(path);
}
