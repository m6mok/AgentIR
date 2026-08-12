use agentir_core::{
    Workspace,
    cpu::{
        CpuArtifactAnchor, CpuArtifactDraft, CpuArtifactPackage, CpuArtifactStore, CpuBinding,
        CpuConstant, CpuExtent, CpuFunction, CpuInstruction, CpuOutput, CpuScalarFunction,
        CpuScalarInstruction, CpuScalarOpcode, CpuScalarOperand, CpuValueType,
        canonical_cpu_artifact_bytes, cpu_artifact_hash,
    },
    ids::{
        CpuArtifactId, MemoryPlanId, MemoryRevisionId, RevisionId, SchedulePlanId,
        ScheduleRevisionId, TargetManifestId, TargetManifestRevisionId, WorkspaceId,
    },
    impl_ir::ImplHash,
    memory::MemoryHash,
    resources::ResourceLimits,
    schedule::ScheduleHash,
    semantic::SpecHash,
    target::TargetHash,
};
use agentir_native_cpu_worker::{
    NATIVE_WORKER_PROTOCOL_VERSION, NativeWorkerRequest, NativeWorkerResult, NativeWorkerSuccess,
    launch_worker_once,
};
use agentir_protocol::Engine;
use agentir_runtime_native_cpu::{NativeWorkerLauncher, ProcessNativeWorkerLauncher};
use agentir_store::load_workspace;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn anchor() -> CpuArtifactAnchor {
    CpuArtifactAnchor {
        spec_revision: RevisionId::new("r1"),
        spec_hash: SpecHash::new("spec"),
        impl_hash: ImplHash::new("impl"),
        memory_hash: MemoryHash::new("memory"),
        memory_plan: MemoryPlanId::new("mp1"),
        memory_revision: MemoryRevisionId::new("mr1"),
        target_hash: TargetHash::new("target"),
        target_manifest: TargetManifestId::new("tm1"),
        target_revision: TargetManifestRevisionId::new("tr1"),
        schedule_hash: ScheduleHash::new("schedule"),
        schedule_plan: SchedulePlanId::new("sp1"),
        schedule_revision: ScheduleRevisionId::new("sr1"),
    }
}

fn publish(
    bindings: Vec<CpuBinding>,
    constants: Vec<CpuConstant>,
    instructions: Vec<CpuInstruction>,
    outputs: Vec<CpuOutput>,
) -> CpuArtifactPackage {
    let register_count = u32::try_from(bindings.len() + instructions.len()).unwrap();
    let mut store = CpuArtifactStore::default();
    store
        .emit(
            SchedulePlanId::new("sp1"),
            ScheduleRevisionId::new("sr1"),
            1,
            CpuArtifactDraft {
                anchor: anchor(),
                bindings,
                constants,
                functions: vec![CpuFunction {
                    name: "main".to_owned(),
                    register_count,
                    instructions,
                }],
                outputs,
            },
        )
        .unwrap();
    store.packages.into_values().next().unwrap()
}

fn scalar_package(opcode: CpuScalarOpcode) -> CpuArtifactPackage {
    let instruction = match opcode {
        CpuScalarOpcode::AddF32 => CpuInstruction::AddF32 {
            output: 3,
            lhs: 0,
            rhs: 1,
        },
        CpuScalarOpcode::MulF32 => CpuInstruction::MulF32 {
            output: 3,
            lhs: 0,
            rhs: 1,
        },
        CpuScalarOpcode::FmaF32 => CpuInstruction::FmaF32 {
            output: 3,
            a: 0,
            b: 1,
            c: 2,
        },
    };
    publish(
        ["a", "b", "c"]
            .into_iter()
            .enumerate()
            .map(|(register, name)| CpuBinding {
                name: name.to_owned(),
                register: u32::try_from(register).unwrap(),
                value_type: CpuValueType::F32,
                extent: None,
            })
            .collect(),
        Vec::new(),
        vec![instruction],
        vec![CpuOutput {
            name: "out".to_owned(),
            register: 3,
            value_type: CpuValueType::F32,
        }],
    )
}

fn tensor_package() -> CpuArtifactPackage {
    let extent = CpuExtent::Symbol {
        name: "N".to_owned(),
    };
    publish(
        vec![
            CpuBinding {
                name: "a".to_owned(),
                register: 0,
                value_type: CpuValueType::F32,
                extent: None,
            },
            CpuBinding {
                name: "x".to_owned(),
                register: 1,
                value_type: CpuValueType::F32Tensor1d,
                extent: Some(extent.clone()),
            },
            CpuBinding {
                name: "y".to_owned(),
                register: 2,
                value_type: CpuValueType::F32Tensor1d,
                extent: Some(extent.clone()),
            },
        ],
        Vec::new(),
        vec![
            CpuInstruction::MapF32 {
                output: 3,
                input: 1,
                extent: extent.clone(),
                body: CpuScalarFunction {
                    arguments: 1,
                    instructions: vec![CpuScalarInstruction {
                        output: 0,
                        opcode: CpuScalarOpcode::MulF32,
                        operands: vec![
                            CpuScalarOperand::Argument { index: 0 },
                            CpuScalarOperand::Capture { register: 0 },
                        ],
                    }],
                    result: CpuScalarOperand::Local { register: 0 },
                },
            },
            CpuInstruction::ZipMapF32 {
                output: 4,
                inputs: vec![1, 2],
                extent,
                body: CpuScalarFunction {
                    arguments: 2,
                    instructions: vec![CpuScalarInstruction {
                        output: 0,
                        opcode: CpuScalarOpcode::FmaF32,
                        operands: vec![
                            CpuScalarOperand::Capture { register: 0 },
                            CpuScalarOperand::Argument { index: 0 },
                            CpuScalarOperand::Argument { index: 1 },
                        ],
                    }],
                    result: CpuScalarOperand::Local { register: 0 },
                },
            },
        ],
        vec![
            CpuOutput {
                name: "map".to_owned(),
                register: 3,
                value_type: CpuValueType::F32Tensor1d,
            },
            CpuOutput {
                name: "zip".to_owned(),
                register: 4,
                value_type: CpuValueType::F32Tensor1d,
            },
        ],
    )
}

fn request(package: CpuArtifactPackage, inputs: BTreeMap<String, Value>) -> NativeWorkerRequest {
    NativeWorkerRequest {
        protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
        expected_cpu_artifact_hash: package.cpu_artifact_hash.clone(),
        package,
        inputs,
    }
}

fn bits(value: &Value) -> u32 {
    (value.as_f64().unwrap() as f32).to_bits()
}

fn native(request: &NativeWorkerRequest) -> NativeWorkerResult<NativeWorkerSuccess> {
    launch_worker_once(
        Path::new(env!("CARGO_BIN_EXE_agentir-native-cpu-worker")),
        request,
    )
}

fn reseal(package: &mut CpuArtifactPackage) {
    package.cpu_artifact_hash = cpu_artifact_hash(package).unwrap();
    package.certificate.cpu_artifact_hash = package.cpu_artifact_hash.clone();
    package.id = CpuArtifactId::new(format!(
        "cpuart-{}",
        &package.cpu_artifact_hash.as_str()[..16]
    ));
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentir-stage9a-{label}-{}-{nonce}.json",
        std::process::id()
    ))
}

#[test]
fn exact_scalar_add_mul_and_fma_match_the_unchanged_interpreter_bitwise() {
    let cases = [
        (
            CpuScalarOpcode::AddF32,
            json!(1.25),
            json!(-0.5),
            json!(0.0),
        ),
        (CpuScalarOpcode::MulF32, json!(-3.0), json!(2.5), json!(0.0)),
        (
            CpuScalarOpcode::FmaF32,
            json!(1.000_000_119_209_289_6),
            json!(1.000_000_119_209_289_6),
            json!(-1.000_000_238_418_579),
        ),
    ];
    for (opcode, a, b, c) in cases {
        let package = scalar_package(opcode);
        let inputs = BTreeMap::from([
            ("a".to_owned(), a),
            ("b".to_owned(), b),
            ("c".to_owned(), c),
        ]);
        let native = native(&request(package.clone(), inputs.clone())).unwrap();
        let interpreted =
            agentir_backend_cpu::execute(&package, &inputs, &ResourceLimits::default()).unwrap();
        assert_eq!(
            bits(&native.outputs["out"]),
            bits(&interpreted.outputs["out"])
        );
    }
}

#[test]
fn tensor_map_zip_map_and_zero_one_multiple_extents_match_bitwise() {
    let package = tensor_package();
    for (x, y) in [
        (json!([]), json!([])),
        (json!([3.0]), json!([4.0])),
        (json!([1.0, 2.0, 3.0, 4.0]), json!([10.0, 20.0, 30.0, 40.0])),
    ] {
        let inputs = BTreeMap::from([
            ("a".to_owned(), json!(2.0)),
            ("x".to_owned(), x),
            ("y".to_owned(), y),
        ]);
        let native = native(&request(package.clone(), inputs.clone())).unwrap();
        let interpreted =
            agentir_backend_cpu::execute(&package, &inputs, &ResourceLimits::default()).unwrap();
        for name in ["map", "zip"] {
            let native = native.outputs[name].as_array().unwrap();
            let interpreted = interpreted.outputs[name].as_array().unwrap();
            assert_eq!(native.len(), interpreted.len());
            assert!(
                native
                    .iter()
                    .zip(interpreted)
                    .all(|(a, b)| bits(a) == bits(b))
            );
        }
    }
}

#[test]
fn native_saxpy_is_exact_and_process_isolated() {
    let package = tensor_package();
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
        ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
    ]);
    let bytes_before = canonical_cpu_artifact_bytes(&package).unwrap();
    let hash_before = package.cpu_artifact_hash.clone();
    let worker = Path::new(env!("CARGO_BIN_EXE_agentir-native-cpu-worker"));
    let response = launch_worker_once(worker, &request(package.clone(), inputs)).unwrap();
    assert_eq!(response.outputs["zip"], json!([12.0, 24.0, 36.0, 48.0]));
    assert_eq!(package.cpu_artifact_hash, hash_before);
    assert_eq!(
        canonical_cpu_artifact_bytes(&package).unwrap(),
        bytes_before
    );
    assert_eq!(response.runtime.cranelift_version, "0.116.1");
    assert_eq!(response.runtime.target_triple, "aarch64-apple-darwin");
}

#[test]
fn retained_production_stage8a_saxpy_package_is_consumed_unchanged() {
    let mut engine = Engine::new();
    for line in include_str!("../../../examples/cpu_saxpy.jsonl").lines() {
        let response: Value = serde_json::from_str(&engine.process_line(line)).unwrap();
        assert_eq!(response["ok"], true, "{response}");
    }
    let path = temp_path("retained-saxpy");
    let save: Value = serde_json::from_str(
        &engine.process_line(
            &json!({
                "command":"workspace.save",
                "request_id":"stage9a-save",
                "workspace":"w1",
                "path":path
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(save["ok"], true, "{save}");
    let loaded = load_workspace(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let package = loaded
        .workspace
        .cpu_artifact_store()
        .packages
        .values()
        .next()
        .unwrap()
        .clone();
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
        ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
    ]);
    let bytes_before = canonical_cpu_artifact_bytes(&package).unwrap();
    let hash_before = package.cpu_artifact_hash.clone();
    let interpreted =
        agentir_backend_cpu::execute(&package, &inputs, &ResourceLimits::default()).unwrap();
    let native = launch_worker_once(
        Path::new(env!("CARGO_BIN_EXE_agentir-native-cpu-worker")),
        &request(package.clone(), inputs),
    )
    .unwrap();
    assert_eq!(native.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
    let native_values = native.outputs["out"].as_array().unwrap();
    let interpreted_values = interpreted.outputs["out"].as_array().unwrap();
    assert!(
        native_values
            .iter()
            .zip(interpreted_values)
            .all(|(a, b)| bits(a) == bits(b))
    );
    assert_eq!(package.cpu_artifact_hash, hash_before);
    assert_eq!(
        canonical_cpu_artifact_bytes(&package).unwrap(),
        bytes_before
    );
}

#[test]
fn signed_zero_finite_edges_and_non_finite_output_are_explicit() {
    let add = scalar_package(CpuScalarOpcode::AddF32);
    let zero_inputs = BTreeMap::from([
        ("a".to_owned(), json!(-0.0)),
        ("b".to_owned(), json!(-0.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let zero = native(&request(add, zero_inputs)).unwrap();
    assert_eq!(bits(&zero.outputs["out"]), (-0.0_f32).to_bits());

    let mul = scalar_package(CpuScalarOpcode::MulF32);
    let finite_inputs = BTreeMap::from([
        ("a".to_owned(), json!(3.402_823_466_385_288_6e38)),
        ("b".to_owned(), json!(1.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let finite = native(&request(mul.clone(), finite_inputs)).unwrap();
    assert_eq!(bits(&finite.outputs["out"]), f32::MAX.to_bits());

    let overflow_inputs = BTreeMap::from([
        ("a".to_owned(), json!(3.402_823_466_385_288_6e38)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    assert!(
        native(&request(mul, overflow_inputs))
            .unwrap_err()
            .to_string()
            .contains("NATIVE_OUTPUT_NON_FINITE")
    );
}

#[test]
fn malformed_coverage_and_stale_or_corrupt_hashes_reject() {
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(1.0)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let mut stale = request(package.clone(), inputs.clone());
    stale.expected_cpu_artifact_hash = agentir_core::cpu::CpuArtifactHash::new("stale");
    assert!(native(&stale).is_err());

    let mut corrupt = request(package.clone(), inputs.clone());
    corrupt.package.functions[0].instructions.clear();
    assert!(native(&corrupt).is_err());

    let mut malformed = package;
    malformed.functions[0].register_count += 1;
    reseal(&mut malformed);
    assert!(
        native(&request(malformed, inputs.clone())).is_err(),
        "incomplete register coverage must reject"
    );

    let mut bad_binding = scalar_package(CpuScalarOpcode::AddF32);
    bad_binding.bindings[0].name.clear();
    reseal(&mut bad_binding);
    assert!(
        native(&request(bad_binding, inputs.clone())).is_err(),
        "malformed binding coverage must reject"
    );

    let mut bad_instruction = scalar_package(CpuScalarOpcode::AddF32);
    let CpuInstruction::AddF32 { rhs, .. } = &mut bad_instruction.functions[0].instructions[0]
    else {
        panic!("add instruction")
    };
    *rhs = 99;
    reseal(&mut bad_instruction);
    assert!(
        native(&request(bad_instruction, inputs.clone())).is_err(),
        "malformed instruction coverage must reject"
    );

    let mut bad_output = scalar_package(CpuScalarOpcode::AddF32);
    bad_output.outputs[0].register = 99;
    reseal(&mut bad_output);
    assert!(
        native(&request(bad_output, inputs)).is_err(),
        "malformed output coverage must reject"
    );
}

#[test]
fn worker_failure_and_malformed_response_exit_without_workspace_mutation() {
    let workspace = Workspace::new(WorkspaceId::new("w1")).unwrap();
    let before = workspace.snapshot();
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let bad_inputs = BTreeMap::from([("a".to_owned(), json!(1.0)), ("b".to_owned(), json!(2.0))]);
    let worker = Path::new(env!("CARGO_BIN_EXE_agentir-native-cpu-worker"));
    assert!(launch_worker_once(worker, &request(package.clone(), bad_inputs)).is_err());
    assert_eq!(workspace.snapshot(), before);

    let malformed_worker = Path::new(env!("CARGO_BIN_EXE_malformed-worker"));
    let valid_inputs = BTreeMap::from([
        ("a".to_owned(), json!(1.0)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let error = launch_worker_once(malformed_worker, &request(package, valid_inputs)).unwrap_err();
    assert!(error.to_string().contains("WORKER_RESPONSE_MALFORMED"));
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn own_unsafe_is_confined_to_the_audited_bridge() {
    let bridge = include_str!("../src/bridge.rs");
    let lowering = include_str!("../src/lowering.rs");
    let library = include_str!("../src/lib.rs");
    let worker = include_str!("../src/main.rs");
    assert_eq!(bridge.matches("unsafe {").count(), 1);
    assert_eq!(bridge.matches("SAFETY:").count(), 1);
    assert!(!lowering.contains("unsafe {"));
    assert!(!library.contains("unsafe {"));
    assert!(!worker.contains("unsafe {"));
    assert!(bridge.contains("abi_version != NATIVE_CALL_ABI_VERSION"));
    assert!(bridge.contains("align_offset"));
}

#[test]
fn constant_instruction_is_covered() {
    let package = publish(
        Vec::new(),
        vec![CpuConstant {
            index: 0,
            bits: "0x3f800000".to_owned(),
        }],
        vec![CpuInstruction::ConstantF32 {
            output: 0,
            constant: 0,
        }],
        vec![CpuOutput {
            name: "out".to_owned(),
            register: 0,
            value_type: CpuValueType::F32,
        }],
    );
    let result = native(&request(package, BTreeMap::new())).unwrap();
    assert_eq!(result.outputs["out"], json!(1.0));
}

#[test]
fn production_launcher_timeout_terminates_without_retry() {
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(1.0)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let mut launcher = ProcessNativeWorkerLauncher::dedicated(env!("CARGO_BIN_EXE_hung-worker"));
    let started = Instant::now();
    let error = launcher
        .launch(&request(package, inputs), Duration::from_millis(20))
        .unwrap_err();
    assert_eq!(
        error.code,
        agentir_core::diagnostics::ErrorCode::CpuNativeWorkerTimeout
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn production_launcher_rejects_stderr_exit_and_extra_data() {
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(1.0)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let request = request(package, inputs);
    for (path, code) in [
        (
            env!("CARGO_BIN_EXE_stderr-worker"),
            agentir_core::diagnostics::ErrorCode::CpuNativeWorkerResponseMalformed,
        ),
        (
            env!("CARGO_BIN_EXE_exit-worker"),
            agentir_core::diagnostics::ErrorCode::CpuNativeWorkerCrashed,
        ),
        (
            env!("CARGO_BIN_EXE_extra-worker"),
            agentir_core::diagnostics::ErrorCode::CpuNativeWorkerResponseMalformed,
        ),
    ] {
        let mut launcher = ProcessNativeWorkerLauncher::dedicated(path);
        assert_eq!(
            launcher
                .launch(&request, Duration::from_secs(5))
                .unwrap_err()
                .code,
            code,
            "fixture {path}"
        );
    }
}

#[test]
fn production_launcher_reports_unavailable_worker_without_retry() {
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(1.0)),
        ("b".to_owned(), json!(2.0)),
        ("c".to_owned(), json!(0.0)),
    ]);
    let missing = std::env::temp_dir().join(format!(
        "agentir-native-worker-does-not-exist-{}",
        std::process::id()
    ));
    let mut launcher = ProcessNativeWorkerLauncher::dedicated(missing);
    assert_eq!(
        launcher
            .launch(&request(package, inputs), Duration::from_millis(50))
            .unwrap_err()
            .code,
        agentir_core::diagnostics::ErrorCode::CpuNativeWorkerUnavailable
    );
}

#[test]
fn native_execution_hash_tracks_inputs_outputs_and_excludes_timeout_policy() {
    let package = scalar_package(CpuScalarOpcode::AddF32);
    let first_request = request(
        package.clone(),
        BTreeMap::from([
            ("a".to_owned(), json!(1.0)),
            ("b".to_owned(), json!(2.0)),
            ("c".to_owned(), json!(0.0)),
        ]),
    );
    let second_request = request(
        package,
        BTreeMap::from([
            ("a".to_owned(), json!(2.0)),
            ("b".to_owned(), json!(2.0)),
            ("c".to_owned(), json!(0.0)),
        ]),
    );
    let worker = env!("CARGO_BIN_EXE_agentir-native-cpu-worker");
    let first = native(&first_request).unwrap();
    let second = native(&second_request).unwrap();
    assert_ne!(
        first.execution.cpu_input_hash,
        second.execution.cpu_input_hash
    );
    assert_ne!(first.execution.output_hash, second.execution.output_hash);
    assert_ne!(
        first.execution.cpu_native_execution_hash,
        second.execution.cpu_native_execution_hash
    );

    let mut launcher = ProcessNativeWorkerLauncher::dedicated(worker);
    let different_timeout = launcher
        .launch(&first_request, Duration::from_secs(2))
        .unwrap();
    assert_eq!(
        first.execution.cpu_native_execution_hash,
        different_timeout.execution.cpu_native_execution_hash
    );
}
