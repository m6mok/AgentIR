use crate::{
    NativeWorkerError, NativeWorkerRequest, NativeWorkerResult, NativeWorkerSuccess, bridge,
};
use agentir_core::cpu::{
    CpuArtifactPackage, CpuExtent, CpuInstruction, CpuScalarFunction, CpuScalarOpcode,
    CpuScalarOperand, CpuValueType, verify_cpu_artifact,
};
use agentir_runtime_native_cpu::{
    CRANELIFT_VERSION, CpuNativeRuntimeIdentity, FIXED_CODEGEN_SETTINGS, NATIVE_CALL_ABI_VERSION,
    NATIVE_RUNTIME_BUILD, NATIVE_WORKER_PROTOCOL_VERSION, build_execution_identity,
    cpu_native_runtime_hash, prepare_native_execution,
};
use cranelift_codegen::{
    ir::{
        AbiParam, InstBuilder, MemFlags, Opcode as ClifOpcode, UserFuncName, Value as ClifValue,
        condcodes::IntCC, types,
    },
    settings::{self, Configurable, SettingKind},
    verifier::verify_function,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use serde_json::{Number, Value};
use std::{collections::BTreeMap, ptr::NonNull};

const MAX_PACKED_VALUES: usize = 1_000_000;
const MAX_LOWERED_INSTRUCTIONS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    offset: usize,
    len: usize,
    value_type: CpuValueType,
}

#[derive(Debug)]
struct Prepared {
    slots: Vec<Slot>,
    packed: Vec<f32>,
    dimensions: BTreeMap<String, usize>,
    instruction_work: u64,
}

fn error(code: &'static str, message: impl Into<String>) -> NativeWorkerError {
    NativeWorkerError::new(code, message)
}

fn finite_f32(value: &Value, context: &str) -> NativeWorkerResult<f32> {
    let number = value
        .as_f64()
        .ok_or_else(|| error("NATIVE_INPUT_TYPE", format!("{context} is not numeric")))?;
    let converted = number as f32;
    if !converted.is_finite() {
        return Err(error(
            "NATIVE_INPUT_NON_FINITE",
            format!("{context} is outside finite f32"),
        ));
    }
    Ok(converted)
}

fn resolve_extent(
    extent: &CpuExtent,
    dimensions: &BTreeMap<String, usize>,
) -> NativeWorkerResult<usize> {
    match extent {
        CpuExtent::Static { value } => usize::try_from(*value)
            .map_err(|_| error("NATIVE_EXTENT_OVERFLOW", "static extent exceeds usize")),
        CpuExtent::Symbol { name } => dimensions.get(name).copied().ok_or_else(|| {
            error(
                "NATIVE_EXTENT_UNBOUND",
                format!("runtime dimension `{name}` is unbound"),
            )
        }),
    }
}

fn set_slot(
    slots: &mut [Option<Slot>],
    register: u32,
    len: usize,
    value_type: CpuValueType,
    total: &mut usize,
) -> NativeWorkerResult<Slot> {
    let index = usize::try_from(register)
        .map_err(|_| error("NATIVE_REGISTER_OVERFLOW", "register exceeds usize"))?;
    let slot = slots.get_mut(index).ok_or_else(|| {
        error(
            "NATIVE_REGISTER_COVERAGE",
            "register is outside declared file",
        )
    })?;
    if slot.is_some() {
        return Err(error(
            "NATIVE_REGISTER_COVERAGE",
            "register is defined more than once",
        ));
    }
    let next = total
        .checked_add(len)
        .ok_or_else(|| error("NATIVE_LAYOUT_OVERFLOW", "packed buffer length overflow"))?;
    if next > MAX_PACKED_VALUES {
        return Err(error(
            "NATIVE_LAYOUT_LIMIT",
            "packed buffer exceeds the Stage 9A worker limit",
        ));
    }
    let value = Slot {
        offset: *total,
        len,
        value_type,
    };
    *slot = Some(value);
    *total = next;
    Ok(value)
}

fn prepare(
    package: &CpuArtifactPackage,
    inputs: &BTreeMap<String, Value>,
) -> NativeWorkerResult<Prepared> {
    verify_cpu_artifact(package).map_err(|error| {
        NativeWorkerError::new(
            "NATIVE_PACKAGE_INVALID",
            format!("independent Stage 8A package verification failed: {error}"),
        )
    })?;
    let function = &package.functions[0];
    let register_count = usize::try_from(function.register_count)
        .map_err(|_| error("NATIVE_REGISTER_OVERFLOW", "register count exceeds usize"))?;
    let mut slots = vec![None; register_count];
    let mut dimensions = BTreeMap::new();
    let mut total = 0_usize;
    let expected_names = package
        .bindings
        .iter()
        .map(|binding| binding.name.as_str())
        .collect::<Vec<_>>();
    let actual_names = inputs.keys().map(String::as_str).collect::<Vec<_>>();
    if expected_names != actual_names {
        return Err(error(
            "NATIVE_INPUT_NAMES",
            "runtime input names do not exactly match ordered package bindings",
        ));
    }
    let mut binding_values = Vec::with_capacity(package.bindings.len());
    for binding in &package.bindings {
        let input = &inputs[&binding.name];
        let values = match binding.value_type {
            CpuValueType::F32 => vec![finite_f32(input, &binding.name)?],
            CpuValueType::F32Tensor1d => {
                let values = input.as_array().ok_or_else(|| {
                    error(
                        "NATIVE_INPUT_TYPE",
                        format!("input `{}` is not a one-dimensional tensor", binding.name),
                    )
                })?;
                let extent = binding.extent.as_ref().ok_or_else(|| {
                    error("NATIVE_PACKAGE_INVALID", "tensor binding has no extent")
                })?;
                match extent {
                    CpuExtent::Static { value }
                        if usize::try_from(*value).ok() == Some(values.len()) => {}
                    CpuExtent::Symbol { name } => {
                        if dimensions
                            .insert(name.clone(), values.len())
                            .is_some_and(|old| old != values.len())
                        {
                            return Err(error(
                                "NATIVE_INPUT_SHAPE",
                                format!("runtime dimension `{name}` is inconsistent"),
                            ));
                        }
                    }
                    CpuExtent::Static { .. } => {
                        return Err(error(
                            "NATIVE_INPUT_SHAPE",
                            format!("input `{}` has an incompatible extent", binding.name),
                        ));
                    }
                }
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| finite_f32(value, &format!("{}[{index}]", binding.name)))
                    .collect::<NativeWorkerResult<Vec<_>>>()?
            }
        };
        let slot = set_slot(
            &mut slots,
            binding.register,
            values.len(),
            binding.value_type,
            &mut total,
        )?;
        binding_values.push((slot, values));
    }
    let mut instruction_work = 0_u64;
    for instruction in &function.instructions {
        let (len, value_type, body_len) = match instruction {
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. }
            | CpuInstruction::FmaF32 { .. } => (1, CpuValueType::F32, 1_u64),
            CpuInstruction::MapF32 { extent, body, .. }
            | CpuInstruction::ZipMapF32 { extent, body, .. } => {
                let len = resolve_extent(extent, &dimensions)?;
                let body = u64::try_from(body.instructions.len())
                    .map_err(|_| error("NATIVE_WORK_OVERFLOW", "scalar body is too large"))?;
                let work = u64::try_from(len)
                    .unwrap_or(u64::MAX)
                    .checked_mul(body)
                    .ok_or_else(|| error("NATIVE_WORK_OVERFLOW", "native work overflow"))?;
                (len, CpuValueType::F32Tensor1d, work)
            }
        };
        instruction_work = instruction_work
            .checked_add(body_len)
            .ok_or_else(|| error("NATIVE_WORK_OVERFLOW", "native work overflow"))?;
        set_slot(
            &mut slots,
            instruction.output(),
            len,
            value_type,
            &mut total,
        )?;
    }
    if instruction_work > MAX_LOWERED_INSTRUCTIONS {
        return Err(error(
            "NATIVE_WORK_LIMIT",
            "native instruction work exceeds the Stage 9A worker limit",
        ));
    }
    let slots = slots
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            error(
                "NATIVE_REGISTER_COVERAGE",
                "register coverage is incomplete",
            )
        })?;
    for output in &package.outputs {
        let slot = slots
            .get(usize::try_from(output.register).unwrap_or(usize::MAX))
            .ok_or_else(|| error("NATIVE_OUTPUT_COVERAGE", "output register is unavailable"))?;
        if slot.value_type != output.value_type {
            return Err(error(
                "NATIVE_OUTPUT_COVERAGE",
                "output type does not match its register",
            ));
        }
    }
    let mut packed = vec![0.0_f32; total];
    for (slot, values) in binding_values {
        packed[slot.offset..slot.offset + slot.len].copy_from_slice(&values);
    }
    Ok(Prepared {
        slots,
        packed,
        dimensions,
        instruction_work,
    })
}

fn byte_offset(offset: usize) -> NativeWorkerResult<i32> {
    offset
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| error("NATIVE_ABI_OFFSET", "packed-buffer byte offset exceeds i32"))
}

fn load_slot(
    builder: &mut FunctionBuilder<'_>,
    base: ClifValue,
    slot: Slot,
) -> NativeWorkerResult<ClifValue> {
    Ok(builder.ins().load(
        types::F32,
        MemFlags::trusted(),
        base,
        byte_offset(slot.offset)?,
    ))
}

fn indexed_address(
    builder: &mut FunctionBuilder<'_>,
    base: ClifValue,
    slot: Slot,
    index: ClifValue,
) -> NativeWorkerResult<ClifValue> {
    let start = builder
        .ins()
        .iadd_imm(base, i64::from(byte_offset(slot.offset)?));
    let scaled = builder.ins().ishl_imm(index, 2);
    Ok(builder.ins().iadd(start, scaled))
}

fn scalar_operand(
    builder: &mut FunctionBuilder<'_>,
    operand: &CpuScalarOperand,
    arguments: &[ClifValue],
    locals: &[ClifValue],
    base: ClifValue,
    slots: &[Slot],
) -> NativeWorkerResult<ClifValue> {
    match operand {
        CpuScalarOperand::Argument { index } => arguments
            .get(usize::try_from(*index).unwrap_or(usize::MAX))
            .copied()
            .ok_or_else(|| error("NATIVE_SCALAR_COVERAGE", "scalar argument is unavailable")),
        CpuScalarOperand::Local { register } => locals
            .get(usize::try_from(*register).unwrap_or(usize::MAX))
            .copied()
            .ok_or_else(|| error("NATIVE_SCALAR_COVERAGE", "scalar local is unavailable")),
        CpuScalarOperand::Capture { register } => {
            let slot = slots
                .get(usize::try_from(*register).unwrap_or(usize::MAX))
                .copied()
                .ok_or_else(|| error("NATIVE_SCALAR_COVERAGE", "capture is unavailable"))?;
            load_slot(builder, base, slot)
        }
    }
}

fn lower_scalar_body(
    builder: &mut FunctionBuilder<'_>,
    body: &CpuScalarFunction,
    arguments: &[ClifValue],
    base: ClifValue,
    slots: &[Slot],
) -> NativeWorkerResult<ClifValue> {
    let mut locals = Vec::with_capacity(body.instructions.len());
    for instruction in &body.instructions {
        let operands = instruction
            .operands
            .iter()
            .map(|operand| scalar_operand(builder, operand, arguments, &locals, base, slots))
            .collect::<NativeWorkerResult<Vec<_>>>()?;
        let value = match instruction.opcode {
            CpuScalarOpcode::AddF32 => builder.ins().fadd(operands[0], operands[1]),
            CpuScalarOpcode::MulF32 => builder.ins().fmul(operands[0], operands[1]),
            CpuScalarOpcode::FmaF32 => builder.ins().fma(operands[0], operands[1], operands[2]),
        };
        locals.push(value);
    }
    scalar_operand(builder, &body.result, arguments, &locals, base, slots)
}

fn expected_fma_instructions(package: &CpuArtifactPackage) -> usize {
    package.functions[0]
        .instructions
        .iter()
        .map(|instruction| match instruction {
            CpuInstruction::FmaF32 { .. } => 1,
            CpuInstruction::MapF32 { body, .. } | CpuInstruction::ZipMapF32 { body, .. } => body
                .instructions
                .iter()
                .filter(|instruction| instruction.opcode == CpuScalarOpcode::FmaF32)
                .count(),
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. } => 0,
        })
        .sum()
}

fn lower_tensor_loop(
    builder: &mut FunctionBuilder<'_>,
    base: ClifValue,
    inputs: &[Slot],
    output: Slot,
    body: &CpuScalarFunction,
    slots: &[Slot],
) -> NativeWorkerResult<()> {
    let header = builder.create_block();
    let body_block = builder.create_block();
    let done = builder.create_block();
    builder.append_block_param(header, types::I64);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().jump(header, &[zero]);
    builder.switch_to_block(header);
    let index = builder.block_params(header)[0];
    let condition = builder.ins().icmp_imm(
        IntCC::UnsignedLessThan,
        index,
        i64::try_from(output.len)
            .map_err(|_| error("NATIVE_EXTENT_OVERFLOW", "tensor extent exceeds i64"))?,
    );
    builder.ins().brif(condition, body_block, &[], done, &[]);
    builder.switch_to_block(body_block);
    let arguments = inputs
        .iter()
        .map(|slot| {
            let address = indexed_address(builder, base, *slot, index)?;
            Ok(builder
                .ins()
                .load(types::F32, MemFlags::trusted(), address, 0))
        })
        .collect::<NativeWorkerResult<Vec<_>>>()?;
    let result = lower_scalar_body(builder, body, &arguments, base, slots)?;
    let address = indexed_address(builder, base, output, index)?;
    builder.ins().store(MemFlags::trusted(), result, address, 0);
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(header, &[next]);
    builder.switch_to_block(done);
    Ok(())
}

fn runtime_identity(module: &JITModule) -> CpuNativeRuntimeIdentity {
    let mut enabled_isa_features = module
        .isa()
        .isa_flags()
        .into_iter()
        .filter(|flag| flag.kind() != SettingKind::Bool || flag.as_bool() == Some(true))
        .map(|flag| format!("{}={}", flag.name, flag.value_string()))
        .collect::<Vec<_>>();
    enabled_isa_features.sort();
    let codegen_settings = FIXED_CODEGEN_SETTINGS
        .map(|(name, value)| format!("{name}={value}"))
        .to_vec();
    let target_triple = module.isa().triple().to_string();
    CpuNativeRuntimeIdentity {
        worker_protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
        runtime_build: NATIVE_RUNTIME_BUILD.to_owned(),
        cranelift_version: CRANELIFT_VERSION.to_owned(),
        target_triple,
        enabled_isa_features,
        codegen_settings,
        abi_version: NATIVE_CALL_ABI_VERSION,
    }
}

fn outputs(
    package: &CpuArtifactPackage,
    prepared: &Prepared,
) -> NativeWorkerResult<BTreeMap<String, Value>> {
    let mut outputs = BTreeMap::new();
    for output in &package.outputs {
        let slot = prepared.slots[usize::try_from(output.register).unwrap_or(usize::MAX)];
        let values = &prepared.packed[slot.offset..slot.offset + slot.len];
        let value = match output.value_type {
            CpuValueType::F32 => number(values[0])?,
            CpuValueType::F32Tensor1d => Value::Array(
                values
                    .iter()
                    .copied()
                    .map(number)
                    .collect::<NativeWorkerResult<Vec<_>>>()?,
            ),
        };
        outputs.insert(output.name.clone(), value);
    }
    Ok(outputs)
}

fn number(value: f32) -> NativeWorkerResult<Value> {
    if !value.is_finite() {
        return Err(error(
            "NATIVE_OUTPUT_NON_FINITE",
            "native execution produced a non-finite f32 output",
        ));
    }
    Number::from_f64(f64::from(value))
        .map(Value::Number)
        .ok_or_else(|| {
            error(
                "NATIVE_OUTPUT_NON_FINITE",
                "native output cannot be encoded",
            )
        })
}

pub(super) fn execute(request: &NativeWorkerRequest) -> NativeWorkerResult<NativeWorkerSuccess> {
    if request.protocol_version != NATIVE_WORKER_PROTOCOL_VERSION {
        return Err(error(
            "NATIVE_PROTOCOL_VERSION",
            "worker protocol version is unsupported",
        ));
    }
    if request.package.cpu_artifact_hash != request.expected_cpu_artifact_hash {
        return Err(error(
            "NATIVE_ARTIFACT_HASH_MISMATCH",
            "expected Stage 8A artifact hash differs from the supplied package",
        ));
    }
    let shared_prepared = prepare_native_execution(
        &request.package,
        &request.expected_cpu_artifact_hash,
        &request.inputs,
        &agentir_core::resources::ResourceLimits::default(),
    )
    .map_err(|error| NativeWorkerError::new("NATIVE_PARENT_VALIDATION", error.to_string()))?;
    let mut prepared = prepare(&request.package, &request.inputs)?;
    let mut flag_builder = settings::builder();
    for (name, value) in FIXED_CODEGEN_SETTINGS {
        flag_builder.set(name, value).map_err(|error| {
            NativeWorkerError::new(
                "NATIVE_CODEGEN_SETTING",
                format!("fixed Cranelift setting `{name}` failed: {error}"),
            )
        })?;
    }
    let isa = cranelift_native::builder()
        .map_err(|message| NativeWorkerError::new("NATIVE_TARGET_UNSUPPORTED", message))?
        .finish(settings::Flags::new(flag_builder))
        .map_err(|error| NativeWorkerError::new("NATIVE_TARGET_UNSUPPORTED", error.to_string()))?;
    let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
    let runtime = runtime_identity(&module);
    let mut context = module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    let mut signature = module.make_signature();
    signature
        .params
        .push(AbiParam::new(module.target_config().pointer_type()));
    let function_id = module
        .declare_function("agentir_native_entry_v1", Linkage::Local, &signature)
        .map_err(|error| NativeWorkerError::new("NATIVE_DECLARE", error.to_string()))?;
    context.func.signature = signature;
    context.func.name = UserFuncName::user(0, function_id.as_u32());
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let base = builder.block_params(entry)[0];
        for instruction in &request.package.functions[0].instructions {
            let output =
                prepared.slots[usize::try_from(instruction.output()).unwrap_or(usize::MAX)];
            match instruction {
                CpuInstruction::ConstantF32 { constant, .. } => {
                    let bits = u32::from_str_radix(
                        request.package.constants[usize::try_from(*constant).unwrap_or(usize::MAX)]
                            .bits
                            .trim_start_matches("0x"),
                        16,
                    )
                    .map_err(|error| {
                        NativeWorkerError::new("NATIVE_CONSTANT", error.to_string())
                    })?;
                    let value = builder
                        .ins()
                        .f32const(cranelift_codegen::ir::immediates::Ieee32::with_bits(bits));
                    builder.ins().store(
                        MemFlags::trusted(),
                        value,
                        base,
                        byte_offset(output.offset)?,
                    );
                }
                CpuInstruction::AddF32 { lhs, rhs, .. }
                | CpuInstruction::MulF32 { lhs, rhs, .. } => {
                    let left = load_slot(
                        &mut builder,
                        base,
                        prepared.slots[usize::try_from(*lhs).unwrap_or(usize::MAX)],
                    )?;
                    let right = load_slot(
                        &mut builder,
                        base,
                        prepared.slots[usize::try_from(*rhs).unwrap_or(usize::MAX)],
                    )?;
                    let value = if matches!(instruction, CpuInstruction::AddF32 { .. }) {
                        builder.ins().fadd(left, right)
                    } else {
                        builder.ins().fmul(left, right)
                    };
                    builder.ins().store(
                        MemFlags::trusted(),
                        value,
                        base,
                        byte_offset(output.offset)?,
                    );
                }
                CpuInstruction::FmaF32 { a, b, c, .. } => {
                    let a = load_slot(
                        &mut builder,
                        base,
                        prepared.slots[usize::try_from(*a).unwrap_or(usize::MAX)],
                    )?;
                    let b = load_slot(
                        &mut builder,
                        base,
                        prepared.slots[usize::try_from(*b).unwrap_or(usize::MAX)],
                    )?;
                    let c = load_slot(
                        &mut builder,
                        base,
                        prepared.slots[usize::try_from(*c).unwrap_or(usize::MAX)],
                    )?;
                    let value = builder.ins().fma(a, b, c);
                    builder.ins().store(
                        MemFlags::trusted(),
                        value,
                        base,
                        byte_offset(output.offset)?,
                    );
                }
                CpuInstruction::MapF32 { input, body, .. } => {
                    lower_tensor_loop(
                        &mut builder,
                        base,
                        &[prepared.slots[usize::try_from(*input).unwrap_or(usize::MAX)]],
                        output,
                        body,
                        &prepared.slots,
                    )?;
                }
                CpuInstruction::ZipMapF32 { inputs, body, .. } => {
                    let inputs = inputs
                        .iter()
                        .map(|input| prepared.slots[usize::try_from(*input).unwrap_or(usize::MAX)])
                        .collect::<Vec<_>>();
                    lower_tensor_loop(&mut builder, base, &inputs, output, body, &prepared.slots)?;
                }
            }
        }
        builder.ins().return_(&[]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    let actual_fma = context
        .func
        .layout
        .blocks()
        .flat_map(|block| context.func.layout.block_insts(block))
        .filter(|instruction| context.func.dfg.insts[*instruction].opcode() == ClifOpcode::Fma)
        .count();
    if actual_fma != expected_fma_instructions(&request.package) {
        return Err(error(
            "NATIVE_FMA_LOWERING",
            "explicit Stage 8A FMA coverage was not preserved as Cranelift fma IR",
        ));
    }
    if !context.func.dfg.ext_funcs.is_empty() || !context.func.global_values.is_empty() {
        return Err(error(
            "NATIVE_EXTERNAL_REFERENCE",
            "Cranelift IR contains a function, symbol, import, or global reference",
        ));
    }
    verify_function(&context.func, module.isa())
        .map_err(|error| NativeWorkerError::new("NATIVE_CRANELIFT_VERIFY", error.to_string()))?;
    module
        .define_function(function_id, &mut context)
        .map_err(|error| NativeWorkerError::new("NATIVE_COMPILE", error.to_string()))?;
    let compiled = context.compiled_code().ok_or_else(|| {
        error(
            "NATIVE_COMPILE",
            "Cranelift did not retain compiled code for relocation audit",
        )
    })?;
    if !compiled.buffer.relocs().is_empty() {
        return Err(error(
            "NATIVE_EXTERNAL_RELOCATION",
            "compiled function requires a symbol, import, or libcall relocation",
        ));
    }
    module
        .finalize_definitions()
        .map_err(|error| NativeWorkerError::new("NATIVE_FINALIZE", error.to_string()))?;
    let code = NonNull::new(module.get_finalized_function(function_id).cast_mut())
        .ok_or_else(|| error("NATIVE_ENTRY_NULL", "finalized native entry point is null"))?;
    bridge::invoke(
        code,
        &mut prepared.packed,
        prepared.slots.iter().map(|slot| slot.len).sum(),
        usize::try_from(module.isa().function_alignment().minimum).map_err(|_| {
            error(
                "NATIVE_ENTRY_ALIGNMENT",
                "target function alignment exceeds usize",
            )
        })?,
        NATIVE_CALL_ABI_VERSION,
    )?;
    let _ = prepared.instruction_work;
    let _ = &prepared.dimensions;
    let outputs = outputs(&request.package, &prepared)?;
    let cpu_native_runtime_hash = cpu_native_runtime_hash(&runtime)
        .map_err(|error| NativeWorkerError::new("NATIVE_RUNTIME_HASH", error.to_string()))?;
    let (_, execution) = build_execution_identity(
        &request.package,
        &cpu_native_runtime_hash,
        &shared_prepared,
        &outputs,
    )
    .map_err(|error| NativeWorkerError::new("NATIVE_EXECUTION_HASH", error.to_string()))?;
    Ok(NativeWorkerSuccess {
        protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
        runtime,
        cpu_native_runtime_hash,
        cpu_artifact_hash: request.package.cpu_artifact_hash.clone(),
        outputs,
        execution,
    })
}
