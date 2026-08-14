use agentir_core::{
    cpu::{CpuInstruction, CpuScalarOpcode},
    diagnostics::{AgentError, ErrorCode},
    ids::WorkspaceId,
    ir::Opcode,
    resources::ResourceLimits,
};
use agentir_protocol::Engine;
use agentir_runtime_native_cpu::{
    CRANELIFT_VERSION, CpuNativeRuntimeIdentity, FIXED_CODEGEN_SETTINGS, NATIVE_CALL_ABI_VERSION,
    NATIVE_RUNTIME_BUILD, NATIVE_WORKER_PROTOCOL_VERSION, NativeCpuPolicy, NativeWorkerLauncher,
    NativeWorkerRequest, NativeWorkerSuccess, PARENT_TARGET_TRIPLE, build_execution_identity,
    cpu_native_runtime_hash, prepare_native_execution,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug)]
enum Behavior {
    Correct,
    Crash,
    WrongArtifact,
    WrongRuntimeHash,
    WrongProtocol,
    WrongAbi,
    WrongCranelift,
    WrongTarget,
    WrongSettings,
    WrongExecution,
    WrongOutputShape,
    MissingOutput,
    ExtraOutput,
}

#[derive(Debug)]
struct State {
    behavior: Behavior,
    calls: usize,
    requests: Vec<NativeWorkerRequest>,
}

#[derive(Clone, Debug)]
struct RecordingLauncher(Arc<Mutex<State>>);

impl NativeWorkerLauncher for RecordingLauncher {
    fn launch(
        &mut self,
        request: &NativeWorkerRequest,
        _timeout: Duration,
    ) -> Result<NativeWorkerSuccess, AgentError> {
        let behavior = {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            state.requests.push(request.clone());
            state.behavior
        };
        if matches!(behavior, Behavior::Crash) {
            return Err(AgentError::new(
                ErrorCode::CpuNativeWorkerCrashed,
                "fixture worker crashed",
            ));
        }
        let prepared = prepare_native_execution(
            &request.package,
            &request.expected_cpu_artifact_hash,
            &request.inputs,
            &ResourceLimits::default(),
        )?;
        let mut runtime = CpuNativeRuntimeIdentity {
            worker_protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
            runtime_build: NATIVE_RUNTIME_BUILD.to_owned(),
            cranelift_version: CRANELIFT_VERSION.to_owned(),
            target_triple: PARENT_TARGET_TRIPLE.to_owned(),
            enabled_isa_features: Vec::new(),
            codegen_settings: FIXED_CODEGEN_SETTINGS
                .map(|(name, value)| format!("{name}={value}"))
                .to_vec(),
            abi_version: NATIVE_CALL_ABI_VERSION,
        };
        match behavior {
            Behavior::WrongAbi => runtime.abi_version += 1,
            Behavior::WrongCranelift => "forged".clone_into(&mut runtime.cranelift_version),
            Behavior::WrongTarget => "forged-target".clone_into(&mut runtime.target_triple),
            Behavior::WrongSettings => runtime.codegen_settings.clear(),
            _ => {}
        }
        let runtime_hash = cpu_native_runtime_hash(&runtime)?;
        let mut outputs = agentir_backend_cpu::execute(
            &request.package,
            &request.inputs,
            &ResourceLimits::default(),
        )?
        .outputs;
        if matches!(behavior, Behavior::WrongOutputShape) {
            outputs.insert("out".to_owned(), json!([12.0]));
        }
        if matches!(behavior, Behavior::MissingOutput) {
            outputs.remove("out");
        }
        if matches!(behavior, Behavior::ExtraOutput) {
            outputs.insert("extra".to_owned(), json!(1.0));
        }
        let (_, mut execution) =
            build_execution_identity(&request.package, &runtime_hash, &prepared, &outputs)?;
        if matches!(behavior, Behavior::WrongExecution) {
            "forged".clone_into(&mut execution.cpu_native_execution_hash);
        }
        Ok(NativeWorkerSuccess {
            protocol_version: if matches!(behavior, Behavior::WrongProtocol) {
                NATIVE_WORKER_PROTOCOL_VERSION + 1
            } else {
                NATIVE_WORKER_PROTOCOL_VERSION
            },
            runtime,
            cpu_native_runtime_hash: if matches!(behavior, Behavior::WrongRuntimeHash) {
                "forged".to_owned()
            } else {
                runtime_hash
            },
            cpu_artifact_hash: if matches!(behavior, Behavior::WrongArtifact) {
                agentir_core::cpu::CpuArtifactHash::new("forged")
            } else {
                request.package.cpu_artifact_hash.clone()
            },
            outputs,
            execution,
        })
    }
}

fn engine(behavior: Behavior) -> (Engine, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        behavior,
        calls: 0,
        requests: Vec::new(),
    }));
    let engine = Engine::with_native_launcher(
        ResourceLimits::default(),
        NativeCpuPolicy::for_tests(Duration::from_millis(50)),
        Box::new(RecordingLauncher(state.clone())),
    );
    (engine, state)
}

fn build_saxpy(engine: &mut Engine) {
    for line in include_str!("../../../examples/cpu_saxpy.jsonl")
        .lines()
        .take(12)
    {
        let response: Value = serde_json::from_str(&engine.process_line(line)).unwrap();
        assert_eq!(response["ok"], true, "{response}");
    }
}

fn native_request() -> Value {
    json!({
        "command": "cpu_native.execute",
        "request_id": "native",
        "workspace": "w1",
        "cpu_artifact": "cpuart-c6eb17c4671f1cb8",
        "expected_cpu_artifact_hash": "c6eb17c4671f1cb8988e92b275357d80a921da61d423bc12211117fef7ea9025",
        "inputs": {"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}
    })
}

fn process(engine: &mut Engine, request: &Value) -> Value {
    serde_json::from_str(&engine.process_line(&request.to_string())).unwrap()
}

fn artifact_query(engine: &mut Engine, request_id: &str) -> Value {
    process(
        engine,
        &json!({
            "command":"cpu_artifact.query",
            "request_id":request_id,
            "workspace":"w1",
            "cpu_artifact":"cpuart-c6eb17c4671f1cb8"
        }),
    )["result"]
        .clone()
}

#[test]
fn production_contract_executes_once_and_leaves_workspace_unchanged() {
    let (mut engine, state) = engine(Behavior::Correct);
    build_saxpy(&mut engine);
    let snapshot_before = engine
        .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
        .unwrap();
    let before = artifact_query(&mut engine, "before");
    let response = process(&mut engine, &native_request());
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["result"]["outputs"]["out"],
        json!([12.0, 24.0, 36.0, 48.0])
    );
    assert_eq!(
        response["result"]["runtime"]["cranelift_version"],
        "0.116.1"
    );
    assert!(response["result"].get("duration").is_none());
    assert!(response["result"].get("cpu_artifact").is_none());
    assert_eq!(artifact_query(&mut engine, "after"), before);
    assert_eq!(
        engine
            .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
            .unwrap(),
        snapshot_before
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls, 1);
    assert_eq!(state.requests.len(), 1);
    assert_eq!(
        state.requests[0].package.cpu_artifact_hash,
        state.requests[0].expected_cpu_artifact_hash
    );
}

#[test]
fn cpu_axpby_fixture_preserves_the_exact_graph_and_workspace() {
    let (mut engine, state) = engine(Behavior::Correct);
    let mut responses = Vec::new();
    let mut before_native = None;
    for line in include_str!("../../../examples/cpu_axpby.jsonl").lines() {
        let request: Value = serde_json::from_str(line).unwrap();
        if request["request_id"] == "native" {
            before_native = Some(
                engine
                    .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
                    .unwrap(),
            );
        }
        let response = process(&mut engine, &request);
        assert_eq!(response["ok"], true, "{response}");
        responses.push(response);
    }

    let output = json!([32.0, 64.0, 96.0, 128.0]);
    for request_id in ["reference", "portable", "native"] {
        let response = responses
            .iter()
            .find(|response| response["request_id"] == request_id)
            .unwrap();
        assert_eq!(response["result"]["outputs"]["out"], output);
    }

    let snapshot = before_native.unwrap();
    let program = &snapshot.revisions.get(&snapshot.head).unwrap().program;
    let zip_maps = program
        .operations
        .values()
        .filter(|operation| operation.opcode == Opcode::ZipMap)
        .collect::<Vec<_>>();
    assert_eq!(zip_maps.len(), 1);
    let source_region = zip_maps[0].region.as_ref().unwrap();
    assert_eq!(source_region.operations.len(), 3);
    assert_eq!(
        source_region
            .operations
            .iter()
            .filter(|operation| operation.opcode == Opcode::Mul)
            .count(),
        2
    );
    assert_eq!(
        source_region
            .operations
            .iter()
            .filter(|operation| operation.opcode == Opcode::Add)
            .count(),
        1
    );
    assert!(
        source_region
            .operations
            .iter()
            .all(|operation| operation.opcode != Opcode::Fma)
    );

    let package = snapshot
        .cpu_artifact_store
        .packages
        .values()
        .next()
        .unwrap();
    assert_eq!(package.functions[0].instructions.len(), 1);
    let CpuInstruction::ZipMapF32 { body, .. } = &package.functions[0].instructions[0] else {
        panic!("AXPBY must retain one zip_map instruction");
    };
    assert_eq!(body.instructions.len(), 3);
    assert_eq!(
        body.instructions
            .iter()
            .filter(|instruction| instruction.opcode == CpuScalarOpcode::MulF32)
            .count(),
        2
    );
    assert_eq!(
        body.instructions
            .iter()
            .filter(|instruction| instruction.opcode == CpuScalarOpcode::AddF32)
            .count(),
        1
    );
    assert!(
        body.instructions
            .iter()
            .all(|instruction| instruction.opcode != CpuScalarOpcode::FmaF32)
    );
    assert_eq!(
        package.certificate.relation,
        "cpu_artifact_equivalent_to_schedule"
    );
    assert_eq!(state.lock().unwrap().calls, 1);
    assert_eq!(
        engine
            .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
            .unwrap(),
        snapshot
    );
}

#[test]
fn every_forbidden_client_field_rejects_before_launch() {
    let forbidden = [
        "package",
        "bytecode",
        "cranelift_ir",
        "machine_code",
        "native_abi",
        "target_triple",
        "cpu_features",
        "codegen_flags",
        "symbols",
        "imports",
        "worker_executable",
        "worker_path",
        "worker_mode",
        "timeout",
        "environment",
        "outputs",
        "runtime_hash",
        "execution_hash",
        "counters",
        "proof",
        "certificate",
        "success",
        "retry",
        "fallback",
        "last",
        "from_command",
    ];
    for field in forbidden {
        let (mut engine, state) = engine(Behavior::Correct);
        let mut request = native_request();
        request
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), json!("forbidden"));
        let response = process(&mut engine, &request);
        assert_eq!(response["ok"], false, "field {field}: {response}");
        assert_eq!(
            response["error"]["code"], "INVALID_REQUEST",
            "field {field}: {response}"
        );
        assert_eq!(state.lock().unwrap().calls, 0);
    }
}

#[test]
fn bad_inputs_and_stale_hash_reject_before_launch() {
    let (mut engine, state) = engine(Behavior::Correct);
    build_saxpy(&mut engine);
    let baseline = engine
        .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
        .unwrap();
    for mutation in [
        json!({"a":2.0,"x":[1.0],"y":[10.0,20.0]}),
        json!({"a":"wrong","x":[1.0],"y":[10.0]}),
        json!({"a":2.0,"x":[1.0],"extra":[10.0]}),
    ] {
        let mut request = native_request();
        request["inputs"] = mutation;
        let response = process(&mut engine, &request);
        assert_eq!(
            response["error"]["code"], "CPU_NATIVE_VALIDATION_FAILED",
            "{response}"
        );
        assert_eq!(
            engine
                .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
                .unwrap(),
            baseline
        );
    }
    let mut stale_request = native_request();
    stale_request["expected_cpu_artifact_hash"] = json!("stale");
    let response = process(&mut engine, &stale_request);
    assert_eq!(
        response["error"]["code"], "CPU_NATIVE_ARTIFACT_IDENTITY_MISMATCH",
        "{response}"
    );
    assert_eq!(
        engine
            .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
            .unwrap(),
        baseline
    );
    assert_eq!(state.lock().unwrap().calls, 0);
}

#[test]
fn projected_work_limit_rejects_before_launch() {
    let (mut engine, state) = engine(Behavior::Correct);
    build_saxpy(&mut engine);
    let limits = ResourceLimits {
        execution_elements: 4,
        ..ResourceLimits::default()
    };
    engine.set_limits_for_tests(limits);
    let before = engine
        .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
        .unwrap();
    let response = process(&mut engine, &native_request());
    assert_eq!(
        response["error"]["code"], "CPU_NATIVE_VALIDATION_FAILED",
        "{response}"
    );
    assert_eq!(
        engine
            .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
            .unwrap(),
        before
    );
    assert_eq!(state.lock().unwrap().calls, 0);
}

#[test]
fn structural_paths_and_portable_execution_never_launch_native_worker() {
    let (mut engine, state) = engine(Behavior::Crash);
    build_saxpy(&mut engine);
    for request in [
        json!({"command":"spec.check","request_id":"spec-check","workspace":"w1"}),
        json!({"command":"program.evaluate","request_id":"reference","workspace":"w1","revision":"r2","inputs":{"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}}),
        json!({"command":"target.check","request_id":"target-check","workspace":"w1","target_manifest":"tm1","target_revision":"tmr1"}),
        json!({"command":"schedule.check","request_id":"schedule-check","workspace":"w1","schedule_plan":"sp1","schedule_revision":"sr1"}),
        json!({"command":"cpu_artifact.list","request_id":"list","workspace":"w1"}),
        json!({"command":"cpu_artifact.query","request_id":"query","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8"}),
        json!({"command":"cpu_artifact.check","request_id":"check","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8","expected_cpu_artifact_hash":"c6eb17c4671f1cb8988e92b275357d80a921da61d423bc12211117fef7ea9025"}),
        json!({"command":"cpu_artifact.execute","request_id":"portable","workspace":"w1","cpu_artifact":"cpuart-c6eb17c4671f1cb8","expected_cpu_artifact_hash":"c6eb17c4671f1cb8988e92b275357d80a921da61d423bc12211117fef7ea9025","inputs":{"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}}),
    ] {
        let response = process(&mut engine, &request);
        assert_eq!(response["ok"], true, "{response}");
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let archive = std::env::temp_dir().join(format!(
        "agentir-native-structural-{}-{nonce}.json",
        std::process::id()
    ));
    let migrated = std::env::temp_dir().join(format!(
        "agentir-native-structural-migrated-{}-{nonce}.json",
        std::process::id()
    ));
    for request in [
        json!({"command":"workspace.save","request_id":"save","workspace":"w1","path":archive.clone()}),
        json!({"command":"workspace.verify_archive","request_id":"verify","path":archive.clone()}),
        json!({"command":"workspace.load","request_id":"load","path":archive.clone(),"replace":true}),
        json!({"command":"workspace.migrate_archive","request_id":"migrate","source_path":archive.clone(),"destination_path":migrated.clone()}),
        json!({"command":"workspace.verify_archive","request_id":"verify-migrated","path":migrated.clone()}),
        json!({"command":"workspace.load","request_id":"load-migrated","path":migrated.clone(),"replace":true}),
    ] {
        let response = process(&mut engine, &request);
        assert_eq!(response["ok"], true, "{response}");
    }
    std::fs::remove_file(archive).unwrap();
    std::fs::remove_file(migrated).unwrap();
    assert_eq!(state.lock().unwrap().calls, 0);
}

#[test]
fn forged_worker_identities_and_outputs_are_typed_and_never_retried() {
    for (behavior, code) in [
        (Behavior::Crash, "CPU_NATIVE_WORKER_CRASHED"),
        (
            Behavior::WrongArtifact,
            "CPU_NATIVE_ARTIFACT_IDENTITY_MISMATCH",
        ),
        (
            Behavior::WrongRuntimeHash,
            "CPU_NATIVE_RUNTIME_IDENTITY_MISMATCH",
        ),
        (
            Behavior::WrongProtocol,
            "CPU_NATIVE_WORKER_PROTOCOL_MISMATCH",
        ),
        (Behavior::WrongAbi, "CPU_NATIVE_RUNTIME_IDENTITY_MISMATCH"),
        (
            Behavior::WrongCranelift,
            "CPU_NATIVE_RUNTIME_IDENTITY_MISMATCH",
        ),
        (
            Behavior::WrongTarget,
            "CPU_NATIVE_RUNTIME_IDENTITY_MISMATCH",
        ),
        (
            Behavior::WrongSettings,
            "CPU_NATIVE_RUNTIME_IDENTITY_MISMATCH",
        ),
        (
            Behavior::WrongExecution,
            "CPU_NATIVE_EXECUTION_IDENTITY_MISMATCH",
        ),
        (Behavior::WrongOutputShape, "CPU_NATIVE_OUTPUT_INVALID"),
        (Behavior::MissingOutput, "CPU_NATIVE_OUTPUT_INVALID"),
        (Behavior::ExtraOutput, "CPU_NATIVE_OUTPUT_INVALID"),
    ] {
        let (mut engine, state) = engine(behavior);
        build_saxpy(&mut engine);
        let snapshot_before = engine
            .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
            .unwrap();
        let before = artifact_query(&mut engine, "before");
        let response = process(&mut engine, &native_request());
        assert_eq!(response["error"]["code"], code, "{behavior:?}: {response}");
        assert_eq!(state.lock().unwrap().calls, 1, "{behavior:?}");
        assert_eq!(artifact_query(&mut engine, "after"), before);
        assert_eq!(
            engine
                .workspace_snapshot_for_tests(&WorkspaceId::new("w1"))
                .unwrap(),
            snapshot_before
        );
    }
}
