use agentir_core::{
    diagnostics::{AgentError, ErrorCode},
    ids::WorkspaceId,
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
    }
    let mut stale_request = native_request();
    stale_request["expected_cpu_artifact_hash"] = json!("stale");
    let response = process(&mut engine, &stale_request);
    assert_eq!(
        response["error"]["code"], "CPU_NATIVE_ARTIFACT_IDENTITY_MISMATCH",
        "{response}"
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
    let response = process(&mut engine, &native_request());
    assert_eq!(
        response["error"]["code"], "CPU_NATIVE_VALIDATION_FAILED",
        "{response}"
    );
    assert_eq!(state.lock().unwrap().calls, 0);
}

#[test]
fn structural_paths_and_portable_execution_never_launch_native_worker() {
    let (mut engine, state) = engine(Behavior::Crash);
    build_saxpy(&mut engine);
    for request in [
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
    for request in [
        json!({"command":"workspace.save","request_id":"save","workspace":"w1","path":archive}),
        json!({"command":"workspace.verify_archive","request_id":"verify","path":archive}),
    ] {
        let response = process(&mut engine, &request);
        assert_eq!(response["ok"], true, "{response}");
    }
    std::fs::remove_file(archive).unwrap();
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
