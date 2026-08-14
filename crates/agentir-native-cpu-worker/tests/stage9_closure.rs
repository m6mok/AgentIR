//! Authoritative offline Stage 9C closure gate.
//!
//! This target composes the focused Stage 9A worker evidence, the production
//! protocol boundary evidence, and the unchanged Stage 8 closure evidence. It
//! adds only closure-level coverage, dependency, unsafe, archive, and process
//! lifecycle audits; it defines no production authority.

include!("stage9a.rs");

mod protocol_evidence {
    include!("../../agentir-protocol/tests/cpu_native.rs");
}

mod stage8_evidence {
    include!("../../agentir-protocol/tests/stage8_closure.rs");
}

use sha2::Digest as _;

fn instruction_tag(instruction: &CpuInstruction) -> &'static str {
    match instruction {
        CpuInstruction::ConstantF32 { .. } => "constant_f32",
        CpuInstruction::AddF32 { .. } => "add_f32",
        CpuInstruction::MulF32 { .. } => "mul_f32",
        CpuInstruction::FmaF32 { .. } => "fma_f32",
        CpuInstruction::MapF32 { .. } => "map_f32",
        CpuInstruction::ZipMapF32 { .. } => "zip_map_f32",
    }
}

fn scalar_opcode_tag(opcode: CpuScalarOpcode) -> &'static str {
    match opcode {
        CpuScalarOpcode::AddF32 => "add_f32",
        CpuScalarOpcode::MulF32 => "mul_f32",
        CpuScalarOpcode::FmaF32 => "fma_f32",
    }
}

fn scalar_operand_tag(operand: &CpuScalarOperand) -> &'static str {
    match operand {
        CpuScalarOperand::Argument { .. } => "argument",
        CpuScalarOperand::Local { .. } => "local",
        CpuScalarOperand::Capture { .. } => "capture",
    }
}

fn collect_package_coverage(
    package: &CpuArtifactPackage,
    instructions: &mut std::collections::BTreeSet<&'static str>,
    scalar_opcodes: &mut std::collections::BTreeSet<&'static str>,
    scalar_operands: &mut std::collections::BTreeSet<&'static str>,
    value_types: &mut std::collections::BTreeSet<&'static str>,
) {
    for binding in &package.bindings {
        value_types.insert(match binding.value_type {
            CpuValueType::F32 => "f32",
            CpuValueType::F32Tensor1d => "f32_tensor_1d",
        });
    }
    for instruction in &package.functions[0].instructions {
        instructions.insert(instruction_tag(instruction));
        let body = match instruction {
            CpuInstruction::MapF32 { body, .. } | CpuInstruction::ZipMapF32 { body, .. } => {
                Some(body)
            }
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. }
            | CpuInstruction::FmaF32 { .. } => None,
        };
        if let Some(body) = body {
            scalar_operands.insert(scalar_operand_tag(&body.result));
            for scalar in &body.instructions {
                scalar_opcodes.insert(scalar_opcode_tag(scalar.opcode));
                for operand in &scalar.operands {
                    scalar_operands.insert(scalar_operand_tag(operand));
                }
            }
        }
    }
}

fn fixed_seed_f32(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    let bounded = i32::try_from(*state % 4097).unwrap() - 2048;
    bounded as f32 / 64.0
}

#[test]
fn fixed_seed_corpus_is_bitwise_and_exhaustive_for_the_current_cpu_enums() {
    let constant = publish(
        Vec::new(),
        vec![CpuConstant {
            index: 0,
            bits: "0x80000000".to_owned(),
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
    let scalar_add = scalar_package(CpuScalarOpcode::AddF32);
    let scalar_mul = scalar_package(CpuScalarOpcode::MulF32);
    let scalar_fma = scalar_package(CpuScalarOpcode::FmaF32);
    let tensor = tensor_package();
    let mut tensor_add = tensor.clone();
    let CpuInstruction::MapF32 { body, .. } = &mut tensor_add.functions[0].instructions[0] else {
        panic!("tensor corpus must start with map_f32");
    };
    body.instructions[0].opcode = CpuScalarOpcode::AddF32;
    reseal(&mut tensor_add);
    let mut instructions = std::collections::BTreeSet::new();
    let mut scalar_opcodes = std::collections::BTreeSet::new();
    let mut scalar_operands = std::collections::BTreeSet::new();
    let mut value_types = std::collections::BTreeSet::new();
    for package in [
        &constant,
        &scalar_add,
        &scalar_mul,
        &scalar_fma,
        &tensor,
        &tensor_add,
    ] {
        collect_package_coverage(
            package,
            &mut instructions,
            &mut scalar_opcodes,
            &mut scalar_operands,
            &mut value_types,
        );
    }
    assert_eq!(
        instructions,
        std::collections::BTreeSet::from([
            "constant_f32",
            "add_f32",
            "mul_f32",
            "fma_f32",
            "map_f32",
            "zip_map_f32",
        ])
    );
    assert_eq!(
        scalar_opcodes,
        std::collections::BTreeSet::from(["add_f32", "mul_f32", "fma_f32"])
    );
    assert_eq!(
        scalar_operands,
        std::collections::BTreeSet::from(["argument", "local", "capture"])
    );
    assert_eq!(
        value_types,
        std::collections::BTreeSet::from(["f32", "f32_tensor_1d"])
    );

    let constant_native = native(&request(constant.clone(), BTreeMap::new())).unwrap();
    let constant_portable =
        agentir_backend_cpu::execute(&constant, &BTreeMap::new(), &ResourceLimits::default())
            .unwrap();
    assert_eq!(
        bits(&constant_native.outputs["out"]),
        bits(&constant_portable.outputs["out"])
    );
    assert_eq!(bits(&constant_native.outputs["out"]), (-0.0_f32).to_bits());

    let mut seed = 0x9c5a_7e11_d3b4_260fu64;
    for package in [&scalar_add, &scalar_mul, &scalar_fma] {
        for _ in 0..8 {
            let inputs = BTreeMap::from([
                ("a".to_owned(), json!(fixed_seed_f32(&mut seed))),
                ("b".to_owned(), json!(fixed_seed_f32(&mut seed))),
                ("c".to_owned(), json!(fixed_seed_f32(&mut seed))),
            ]);
            let native_result = native(&request((*package).clone(), inputs.clone())).unwrap();
            let portable =
                agentir_backend_cpu::execute(package, &inputs, &ResourceLimits::default()).unwrap();
            assert_eq!(
                bits(&native_result.outputs["out"]),
                bits(&portable.outputs["out"])
            );
        }
    }

    let tensor_inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([-0.0, 1.0, -3.5])),
        ("y".to_owned(), json!([0.0, 4.0, 2.5])),
    ]);
    let tensor_add_native = native(&request(tensor_add.clone(), tensor_inputs.clone())).unwrap();
    let tensor_add_portable =
        agentir_backend_cpu::execute(&tensor_add, &tensor_inputs, &ResourceLimits::default())
            .unwrap();
    for name in ["map", "zip"] {
        assert!(
            tensor_add_native.outputs[name]
                .as_array()
                .unwrap()
                .iter()
                .zip(tensor_add_portable.outputs[name].as_array().unwrap())
                .all(|(left, right)| bits(left) == bits(right))
        );
    }

    let mut static_tensor = tensor.clone();
    for binding in &mut static_tensor.bindings {
        if binding.value_type == CpuValueType::F32Tensor1d {
            binding.extent = Some(CpuExtent::Static { value: 1 });
        }
    }
    for instruction in &mut static_tensor.functions[0].instructions {
        match instruction {
            CpuInstruction::MapF32 { extent, .. } | CpuInstruction::ZipMapF32 { extent, .. } => {
                *extent = CpuExtent::Static { value: 1 };
            }
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. }
            | CpuInstruction::FmaF32 { .. } => {}
        }
    }
    reseal(&mut static_tensor);
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([3.0])),
        ("y".to_owned(), json!([4.0])),
    ]);
    let native_result = native(&request(static_tensor.clone(), inputs.clone())).unwrap();
    let portable =
        agentir_backend_cpu::execute(&static_tensor, &inputs, &ResourceLimits::default()).unwrap();
    for name in ["map", "zip"] {
        assert!(
            native_result.outputs[name]
                .as_array()
                .unwrap()
                .iter()
                .zip(portable.outputs[name].as_array().unwrap())
                .all(|(left, right)| bits(left) == bits(right))
        );
    }
}

#[test]
fn closure_structurally_pins_dependencies_unsafe_reaping_and_legacy_archives() {
    let protocol_manifest = include_str!("../../agentir-protocol/Cargo.toml");
    let parent_manifest = include_str!("../../agentir-runtime-native-cpu/Cargo.toml");
    let worker_manifest = include_str!("../Cargo.toml");
    assert!(!protocol_manifest.contains("cranelift-"));
    assert!(!parent_manifest.contains("cranelift-"));
    for dependency in [
        "cranelift-codegen.workspace = true",
        "cranelift-frontend.workspace = true",
        "cranelift-jit.workspace = true",
        "cranelift-module.workspace = true",
        "cranelift-native.workspace = true",
    ] {
        assert!(worker_manifest.contains(dependency));
    }

    let lock = include_str!("../../../Cargo.lock");
    for package in [
        "cranelift-codegen",
        "cranelift-frontend",
        "cranelift-jit",
        "cranelift-module",
        "cranelift-native",
    ] {
        assert!(lock.contains(&format!("name = \"{package}\"\nversion = \"0.116.1\"")));
    }

    let runtime = include_str!("../../agentir-runtime-native-cpu/src/lib.rs");
    assert!(runtime.contains("fn terminate_and_reap(child: &mut Child)"));
    assert!(runtime.contains("let _ = child.kill();\n    let _ = child.wait();"));
    assert!(runtime.contains("match child.try_wait()"));
    assert!(runtime.contains("check_exit(status)?;"));
    assert_eq!(runtime.matches("Instant::now()").count(), 1);

    let bridge = include_str!("../src/bridge.rs");
    assert_eq!(bridge.matches("unsafe {").count(), 1);
    assert_eq!(bridge.matches("SAFETY:").count(), 1);
    for source in [
        include_str!("../../agentir-core/src/lib.rs"),
        include_str!("../../agentir-store/src/lib.rs"),
        include_str!("../../agentir-protocol/src/lib.rs"),
        include_str!("../../agentir-backend-cpu/src/lib.rs"),
        include_str!("../../agentir-runtime-cpu/src/lib.rs"),
        include_str!("../../agentir-runtime-native-cpu/src/lib.rs"),
    ] {
        assert!(source.contains("#![forbid(unsafe_code)]"));
    }

    for (bytes, version, expected_hash) in [
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v1.json").as_slice(),
            1,
            "6179d2f90d204e51fcbc237f51a4f8695af3f0908d6ac4759b46eab57d3399db",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v2.json").as_slice(),
            2,
            "1e8a5a04317a5e3fbcc96fbd25ccc9b733b52ad15254aa30f98244ac9c8e8b4c",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v3.json").as_slice(),
            3,
            "b929554e6b5981695fead2fd5b2fa9425f1718ed41eeab3ce6e83252836a9983",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v4.json").as_slice(),
            4,
            "2975f4a4be4977b182a52a46b5b9e4708635a495b0d45ff901b96eaabff467da",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v5.json").as_slice(),
            5,
            "315f39f987119285e7e441962515ef1a09bbb384686fbb989e8b49665906bf17",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v6.json").as_slice(),
            6,
            "37b3ce979c93cc55e4ac78b5d85be8639eded96ef43af98ffde24f8bd2f53e7f",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v7.json").as_slice(),
            7,
            "67277adac68aad413ea7f440068924ef80b1f8a5e7bb87db5543516a404b4f8d",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v8.json").as_slice(),
            8,
            "c90b655b840e83b53a60ddbb5ef2508ea2c69f7b0092858d0954ae14f8425f39",
        ),
        (
            include_bytes!("../../agentir-store/tests/fixtures/minimal-v9.json").as_slice(),
            9,
            "ae6b35d34607a425f466f561c004928b2df8ca7783ade416e95f2227641648c8",
        ),
    ] {
        assert_eq!(format!("{:x}", sha2::Sha256::digest(bytes)), expected_hash);
        let loaded = agentir_store::load_workspace_bytes(bytes).unwrap();
        assert_eq!(loaded.metadata.format_version, version);
    }
}
