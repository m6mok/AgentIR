//! Compiler-owned portable CPU artifacts and deterministic structural checks.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        CpuArtifactId, MemoryPlanId, MemoryRevisionId, RevisionId, SchedulePlanId,
        ScheduleRevisionId, TargetManifestId, TargetManifestRevisionId,
    },
    impl_ir::ImplHash,
    memory::MemoryHash,
    schedule::ScheduleHash,
    semantic::SpecHash,
    target::TargetHash,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fmt::Write as _,
};

/// Portable scalar bytecode format version.
pub const CPU_ARTIFACT_FORMAT_VERSION: u32 = 1;
/// Structural CPU artifact validator version.
pub const CPU_ARTIFACT_VALIDATOR_VERSION: u32 = 1;
/// CPU artifact event replay semantics version.
pub const CPU_ARTIFACT_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for CPU compiler build identity.
pub const CPU_COMPILER_BUILD_HASH_DOMAIN: &[u8] = b"agentir.compiler.cpu_scalar.v1\0";
/// Domain separator for portable scalar CPU artifact identity.
pub const CPU_ARTIFACT_HASH_DOMAIN: &[u8] = b"agentir.artifact.cpu.scalar.v1\0";

const CPU_ARTIFACT_CERTIFICATE_CONDITIONS: [&str; 5] = [
    "immutable_stage_1_4_anchor_chain_verified",
    "serial_schedule_coverage_preserved",
    "one_dimensional_f32_binding_types_verified",
    "ordered_scalar_bytecode_lowering_verified",
    "compiler_owned_bounds_validation_retained",
];

macro_rules! hash_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a hash from lowercase hexadecimal text.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns lowercase hexadecimal text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

hash_type!(
    CpuArtifactHash,
    "Exact identity of one portable scalar CPU package."
);
hash_type!(
    CpuCompilerBuildHash,
    "Identity of the deterministic CPU compiler contract."
);

fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn cpu_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

/// Current deterministic CPU compiler build identity.
#[must_use]
pub fn cpu_compiler_build_hash() -> CpuCompilerBuildHash {
    let model = format!(
        "agentir:{}:cpu_artifact={}:validator={}",
        env!("CARGO_PKG_VERSION"),
        CPU_ARTIFACT_FORMAT_VERSION,
        CPU_ARTIFACT_VALIDATOR_VERSION
    );
    CpuCompilerBuildHash(digest(CPU_COMPILER_BUILD_HASH_DOMAIN, model.as_bytes()))
}

/// Immutable Stage 1-4 anchor retained by one CPU package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactAnchor {
    /// Frozen SpecIR revision identity.
    pub spec_revision: RevisionId,
    /// Frozen SpecIR semantic identity.
    pub spec_hash: SpecHash,
    /// Reachable ImplIR semantic identity.
    pub impl_hash: ImplHash,
    /// Exact MemoryIR identity.
    pub memory_hash: MemoryHash,
    /// Anchored MemoryIR plan.
    pub memory_plan: MemoryPlanId,
    /// Anchored MemoryIR revision.
    pub memory_revision: MemoryRevisionId,
    /// Immutable CPU target identity.
    pub target_hash: TargetHash,
    /// Anchored target manifest.
    pub target_manifest: TargetManifestId,
    /// Anchored target revision.
    pub target_revision: TargetManifestRevisionId,
    /// Exact ScheduleIR identity.
    pub schedule_hash: ScheduleHash,
    /// Anchored ScheduleIR plan.
    pub schedule_plan: SchedulePlanId,
    /// Anchored ScheduleIR revision.
    pub schedule_revision: ScheduleRevisionId,
}

/// Exact one-dimensional extent encoded by the CPU package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CpuExtent {
    /// Fully static element count.
    Static {
        /// Exact non-negative element count.
        value: u64,
    },
    /// Runtime dimension inferred and checked from input bindings.
    Symbol {
        /// Stable symbolic dimension name.
        name: String,
    },
}

/// Runtime value kind accepted by one binding or register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuValueType {
    /// IEEE-754 binary32 scalar.
    F32,
    /// Dense one-dimensional IEEE-754 binary32 tensor.
    F32Tensor1d,
}

/// One ordered external input binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuBinding {
    /// Stable external parameter name.
    pub name: String,
    /// Canonical bytecode register populated before execution.
    pub register: u32,
    /// Exact runtime value kind.
    pub value_type: CpuValueType,
    /// Tensor extent, absent for scalar bindings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<CpuExtent>,
}

/// One ordered canonical f32 constant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuConstant {
    /// Zero-based constant table index.
    pub index: u32,
    /// Exact lowercase IEEE-754 binary32 bits (`0x` plus eight hex digits).
    pub bits: String,
}

/// Operand inside a scalar elementwise function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CpuScalarOperand {
    /// Elementwise function argument.
    Argument {
        /// Zero-based argument index.
        index: u32,
    },
    /// Earlier scalar local result.
    Local {
        /// Zero-based local register.
        register: u32,
    },
    /// Scalar top-level bytecode register captured by the function.
    Capture {
        /// Top-level scalar f32 register.
        register: u32,
    },
}

/// Scalar opcode supported by `cpu_scalar_v1` elementwise functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuScalarOpcode {
    /// Exact ordered f32 addition.
    AddF32,
    /// Exact ordered f32 multiplication.
    MulF32,
    /// Explicit fused f32 multiply-add.
    FmaF32,
}

/// One ordered scalar function instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuScalarInstruction {
    /// Zero-based local result register.
    pub output: u32,
    /// Scalar operation.
    pub opcode: CpuScalarOpcode,
    /// Ordered operands.
    pub operands: Vec<CpuScalarOperand>,
}

/// Closed scalar function used by one map instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuScalarFunction {
    /// Number of ordered element arguments.
    pub arguments: u32,
    /// Ordered local instructions.
    pub instructions: Vec<CpuScalarInstruction>,
    /// Returned argument/local/capture value.
    pub result: CpuScalarOperand,
}

/// Portable top-level scalar CPU instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "opcode", rename_all = "snake_case")]
pub enum CpuInstruction {
    /// Loads one canonical constant into a scalar register.
    ConstantF32 {
        /// Result register.
        output: u32,
        /// Constant-table index.
        constant: u32,
    },
    /// Adds two scalar f32 registers.
    AddF32 {
        /// Result register.
        output: u32,
        /// Ordered left operand register.
        lhs: u32,
        /// Ordered right operand register.
        rhs: u32,
    },
    /// Multiplies two scalar f32 registers.
    MulF32 {
        /// Result register.
        output: u32,
        /// Ordered left operand register.
        lhs: u32,
        /// Ordered right operand register.
        rhs: u32,
    },
    /// Performs explicit fused multiply-add on scalar f32 registers.
    FmaF32 {
        /// Result register.
        output: u32,
        /// Multiplicand register.
        a: u32,
        /// Multiplier register.
        b: u32,
        /// Addend register.
        c: u32,
    },
    /// Applies a closed scalar function serially to one tensor.
    MapF32 {
        /// Result tensor register.
        output: u32,
        /// Input tensor register.
        input: u32,
        /// Exact serial iteration extent.
        extent: CpuExtent,
        /// Closed scalar element function.
        body: CpuScalarFunction,
    },
    /// Applies a closed scalar function serially to ordered equal-shape tensors.
    ZipMapF32 {
        /// Result tensor register.
        output: u32,
        /// Ordered input tensor registers.
        inputs: Vec<u32>,
        /// Exact serial iteration extent.
        extent: CpuExtent,
        /// Closed scalar element function.
        body: CpuScalarFunction,
    },
}

impl CpuInstruction {
    /// Returns the result register defined by this instruction.
    #[must_use]
    pub const fn output(&self) -> u32 {
        match self {
            Self::ConstantF32 { output, .. }
            | Self::AddF32 { output, .. }
            | Self::MulF32 { output, .. }
            | Self::FmaF32 { output, .. }
            | Self::MapF32 { output, .. }
            | Self::ZipMapF32 { output, .. } => *output,
        }
    }
}

/// One deterministic bytecode entry function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuFunction {
    /// Stable function name; v1 contains exactly `main`.
    pub name: String,
    /// Total register count after all bindings and instructions.
    pub register_count: u32,
    /// Ordered bytecode instructions.
    pub instructions: Vec<CpuInstruction>,
}

/// One named output mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuOutput {
    /// Stable external output name.
    pub name: String,
    /// Bytecode register returned for this output.
    pub register: u32,
    /// Exact output type.
    pub value_type: CpuValueType,
}

/// Lifecycle state of one CPU package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuArtifactStatus {
    /// Compiler lowering completed and structural validation passed.
    Validated,
}

/// Compiler-owned structural proof from one schedule to one CPU package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactCertificate {
    /// Stable proof relation.
    pub relation: String,
    /// Anchored exact schedule identity.
    pub schedule_hash: ScheduleHash,
    /// Exact CPU artifact identity.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Ordered verified conditions.
    pub conditions: Vec<String>,
    /// Structural validator version.
    pub validator_version: u32,
}

/// Deterministic portable scalar CPU package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactPackage {
    /// Compiler-assigned content identity.
    pub id: CpuArtifactId,
    /// Stable package discriminator.
    pub format: String,
    /// Portable bytecode version.
    pub format_version: u32,
    /// Stable compiler-owned target profile.
    pub target_profile: String,
    /// Exact compiler pipeline anchors.
    pub anchor: CpuArtifactAnchor,
    /// Separate CPU compiler build identity.
    pub compiler_build_hash: CpuCompilerBuildHash,
    /// Ordered external bindings.
    pub bindings: Vec<CpuBinding>,
    /// Ordered canonical constants.
    pub constants: Vec<CpuConstant>,
    /// Ordered bytecode functions.
    pub functions: Vec<CpuFunction>,
    /// Ordered named outputs.
    pub outputs: Vec<CpuOutput>,
    /// Offline validation state.
    pub status: CpuArtifactStatus,
    /// Independent exact package hash.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Compiler-owned structural certificate.
    pub certificate: CpuArtifactCertificate,
}

/// Trusted lowering result before compiler-owned identity and proof publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuArtifactDraft {
    /// Exact compiler pipeline anchors.
    pub anchor: CpuArtifactAnchor,
    /// Ordered external bindings.
    pub bindings: Vec<CpuBinding>,
    /// Ordered canonical constants.
    pub constants: Vec<CpuConstant>,
    /// Ordered bytecode functions.
    pub functions: Vec<CpuFunction>,
    /// Ordered named outputs.
    pub outputs: Vec<CpuOutput>,
}

#[derive(Serialize)]
struct CpuArtifactHashModel<'a> {
    format: &'a str,
    format_version: u32,
    target_profile: &'a str,
    anchor: &'a CpuArtifactAnchor,
    compiler_build_hash: &'a CpuCompilerBuildHash,
    bindings: &'a [CpuBinding],
    constants: &'a [CpuConstant],
    functions: &'a [CpuFunction],
    outputs: &'a [CpuOutput],
    status: CpuArtifactStatus,
    certificate_relation: &'a str,
    certificate_conditions: &'a [String],
    validator_version: u32,
}

/// Returns deterministic semantic package bytes, excluding only the derived ID/hash fields.
pub fn canonical_cpu_artifact_bytes(package: &CpuArtifactPackage) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(&CpuArtifactHashModel {
        format: &package.format,
        format_version: package.format_version,
        target_profile: &package.target_profile,
        anchor: &package.anchor,
        compiler_build_hash: &package.compiler_build_hash,
        bindings: &package.bindings,
        constants: &package.constants,
        functions: &package.functions,
        outputs: &package.outputs,
        status: package.status,
        certificate_relation: &package.certificate.relation,
        certificate_conditions: &package.certificate.conditions,
        validator_version: package.certificate.validator_version,
    })
    .map_err(|error| {
        cpu_error(
            ErrorCode::CanonicalizationFailed,
            format!("CPU artifact canonicalization failed: {error}"),
        )
    })
}

/// Recomputes the independent CPU artifact hash.
pub fn cpu_artifact_hash(package: &CpuArtifactPackage) -> AgentResult<CpuArtifactHash> {
    Ok(CpuArtifactHash(digest(
        CPU_ARTIFACT_HASH_DOMAIN,
        &canonical_cpu_artifact_bytes(package)?,
    )))
}

fn check_scalar_operand(
    operand: &CpuScalarOperand,
    arguments: u32,
    locals: u32,
    top_types: &BTreeMap<u32, CpuValueType>,
) -> bool {
    match operand {
        CpuScalarOperand::Argument { index } => *index < arguments,
        CpuScalarOperand::Local { register } => *register < locals,
        CpuScalarOperand::Capture { register } => {
            top_types.get(register) == Some(&CpuValueType::F32)
        }
    }
}

fn verify_scalar_function(
    body: &CpuScalarFunction,
    expected_arguments: u32,
    top_types: &BTreeMap<u32, CpuValueType>,
) -> AgentResult<()> {
    if body.arguments != expected_arguments {
        return Err(cpu_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU scalar function argument count differs from its map instruction",
        ));
    }
    for (index, instruction) in body.instructions.iter().enumerate() {
        let expected = u32::try_from(index).unwrap_or(u32::MAX);
        let arity = match instruction.opcode {
            CpuScalarOpcode::AddF32 | CpuScalarOpcode::MulF32 => 2,
            CpuScalarOpcode::FmaF32 => 3,
        };
        if instruction.output != expected
            || instruction.operands.len() != arity
            || instruction
                .operands
                .iter()
                .any(|operand| !check_scalar_operand(operand, body.arguments, expected, top_types))
        {
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU scalar function is not ordered, closed, or well typed",
            ));
        }
    }
    let locals = u32::try_from(body.instructions.len()).unwrap_or(u32::MAX);
    if !check_scalar_operand(&body.result, body.arguments, locals, top_types) {
        return Err(cpu_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU scalar function result is unavailable",
        ));
    }
    Ok(())
}

fn valid_extent(extent: &CpuExtent) -> bool {
    match extent {
        CpuExtent::Static { .. } => true,
        CpuExtent::Symbol { name } => !name.is_empty(),
    }
}

/// Structurally verifies one portable CPU package without executing bytecode.
pub fn verify_cpu_artifact(package: &CpuArtifactPackage) -> AgentResult<()> {
    if package.format != "agentir.cpu.scalar.package"
        || package.format_version != CPU_ARTIFACT_FORMAT_VERSION
        || package.target_profile != "cpu_scalar_v1"
        || package.compiler_build_hash != cpu_compiler_build_hash()
        || package.status != CpuArtifactStatus::Validated
    {
        return Err(cpu_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU artifact format, profile, build, or lifecycle is invalid",
        ));
    }
    let mut previous_name: Option<&str> = None;
    let mut top_types = BTreeMap::new();
    let mut top_extents = BTreeMap::new();
    for binding in &package.bindings {
        if binding.name.is_empty()
            || previous_name.is_some_and(|previous| previous >= binding.name.as_str())
            || top_types
                .insert(binding.register, binding.value_type)
                .is_some()
            || match binding.value_type {
                CpuValueType::F32 => binding.extent.is_some(),
                CpuValueType::F32Tensor1d => binding
                    .extent
                    .as_ref()
                    .is_none_or(|extent| !valid_extent(extent)),
            }
        {
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU bindings are not uniquely name-ordered, typed, and registered",
            ));
        }
        if let Some(extent) = &binding.extent {
            top_extents.insert(binding.register, extent.clone());
        }
        previous_name = Some(&binding.name);
    }
    for (index, constant) in package.constants.iter().enumerate() {
        let valid_bits = constant.bits.len() == 10
            && constant.bits.starts_with("0x")
            && constant.bits[2..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if constant.index != u32::try_from(index).unwrap_or(u32::MAX) || !valid_bits {
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU constant table is not canonical and contiguous",
            ));
        }
    }
    if package.functions.len() != 1 || package.functions[0].name != "main" {
        return Err(cpu_error(
            ErrorCode::CpuBytecodeUnsupported,
            "CPU artifact v1 requires exactly one `main` function",
        ));
    }
    let function = &package.functions[0];
    for instruction in &function.instructions {
        let output = instruction.output();
        if top_types.contains_key(&output) {
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU bytecode register is defined more than once",
            ));
        }
        let require = |register: &u32, expected: CpuValueType| {
            top_types.get(register).copied() == Some(expected)
        };
        let (output_type, output_extent) = match instruction {
            CpuInstruction::ConstantF32 { constant, .. } => {
                if usize::try_from(*constant)
                    .ok()
                    .and_then(|index| package.constants.get(index))
                    .is_none()
                {
                    return Err(cpu_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU bytecode references a missing constant",
                    ));
                }
                (CpuValueType::F32, None)
            }
            CpuInstruction::AddF32 { lhs, rhs, .. } | CpuInstruction::MulF32 { lhs, rhs, .. } => {
                if !require(lhs, CpuValueType::F32) || !require(rhs, CpuValueType::F32) {
                    return Err(cpu_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU scalar arithmetic references a missing or non-f32 register",
                    ));
                }
                (CpuValueType::F32, None)
            }
            CpuInstruction::FmaF32 { a, b, c, .. } => {
                if !require(a, CpuValueType::F32)
                    || !require(b, CpuValueType::F32)
                    || !require(c, CpuValueType::F32)
                {
                    return Err(cpu_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU fma references a missing or non-f32 register",
                    ));
                }
                (CpuValueType::F32, None)
            }
            CpuInstruction::MapF32 {
                input,
                extent,
                body,
                ..
            } => {
                if !valid_extent(extent)
                    || !require(input, CpuValueType::F32Tensor1d)
                    || top_extents.get(input) != Some(extent)
                {
                    return Err(cpu_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU map input is missing or has an incompatible one-dimensional extent",
                    ));
                }
                verify_scalar_function(body, 1, &top_types)?;
                (CpuValueType::F32Tensor1d, Some(extent.clone()))
            }
            CpuInstruction::ZipMapF32 {
                inputs,
                extent,
                body,
                ..
            } => {
                if inputs.is_empty()
                    || !valid_extent(extent)
                    || inputs.iter().any(|input| {
                        !require(input, CpuValueType::F32Tensor1d)
                            || top_extents.get(input) != Some(extent)
                    })
                {
                    return Err(cpu_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU zip_map inputs are missing or have incompatible extents",
                    ));
                }
                verify_scalar_function(
                    body,
                    u32::try_from(inputs.len()).unwrap_or(u32::MAX),
                    &top_types,
                )?;
                (CpuValueType::F32Tensor1d, Some(extent.clone()))
            }
        };
        top_types.insert(output, output_type);
        if let Some(extent) = output_extent {
            top_extents.insert(output, extent);
        }
    }
    if u32::try_from(top_types.len()).unwrap_or(u32::MAX) != function.register_count
        || top_types.keys().copied().collect::<BTreeSet<_>>()
            != (0..function.register_count).collect::<BTreeSet<_>>()
    {
        return Err(cpu_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU register file is not dense and exactly sized",
        ));
    }
    let mut previous_output: Option<&str> = None;
    for output in &package.outputs {
        if output.name.is_empty()
            || previous_output.is_some_and(|previous| previous >= output.name.as_str())
            || top_types.get(&output.register) != Some(&output.value_type)
        {
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU outputs are not uniquely name-ordered or well typed",
            ));
        }
        previous_output = Some(&output.name);
    }
    if package.outputs.is_empty()
        || package.certificate.relation != "cpu_artifact_equivalent_to_schedule"
        || package.certificate.schedule_hash != package.anchor.schedule_hash
        || package.certificate.cpu_artifact_hash != package.cpu_artifact_hash
        || package.certificate.validator_version != CPU_ARTIFACT_VALIDATOR_VERSION
        || package.certificate.conditions != CPU_ARTIFACT_CERTIFICATE_CONDITIONS.map(str::to_owned)
    {
        return Err(cpu_error(
            ErrorCode::CpuArtifactEquivalenceUnproved,
            "CpuArtifactEquivalentToSchedule certificate is missing or inconsistent",
        ));
    }
    let actual = cpu_artifact_hash(package)?;
    if actual != package.cpu_artifact_hash {
        return Err(cpu_error(
            ErrorCode::CpuArtifactHashMismatch,
            "CPU artifact hash does not match its exact portable package",
        )
        .with_types(package.cpu_artifact_hash.to_string(), actual.to_string()));
    }
    let expected_id = CpuArtifactId::new(format!(
        "cpuart-{}",
        &package.cpu_artifact_hash.as_str()[..16]
    ));
    if package.id != expected_id {
        return Err(cpu_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU artifact content identity does not match its package hash",
        ));
    }
    Ok(())
}

/// Deterministic CPU artifact summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactQuery {
    /// Compiler-owned package identity.
    pub cpu_artifact: CpuArtifactId,
    /// Exact package hash.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Exact source schedule hash.
    pub schedule_hash: ScheduleHash,
    /// Exact CPU target hash.
    pub target_hash: TargetHash,
    /// CPU compiler build identity.
    pub compiler_build_hash: CpuCompilerBuildHash,
    /// Number of external bindings.
    pub binding_count: usize,
    /// Number of canonical constants.
    pub constant_count: usize,
    /// Number of portable instructions.
    pub instruction_count: usize,
    /// Canonical semantic package byte count.
    pub canonical_bytes: usize,
    /// Offline lifecycle state.
    pub status: CpuArtifactStatus,
}

/// Full structural CPU artifact validation report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactCheckReport {
    /// Deterministic package summary.
    pub query: CpuArtifactQuery,
    /// Whether structural bytecode validation passed.
    pub offline_valid: bool,
    /// Whether compiler-owned proof binds the package to ScheduleIR.
    pub equivalent_to_schedule: bool,
}

/// Replayable CPU artifact publication event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactEvent {
    /// Source schedule plan.
    pub schedule_plan: SchedulePlanId,
    /// Source schedule revision.
    pub schedule_revision: ScheduleRevisionId,
    /// Exact compiler-published package.
    pub package: CpuArtifactPackage,
    /// Schedule event dependency cursor.
    pub schedule_event_cursor: u64,
}

/// CPU artifact event with independent replay semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedCpuArtifactEvent {
    /// CPU artifact event semantics version.
    pub semantics_version: u32,
    /// Replayable publication event.
    pub event: CpuArtifactEvent,
}

/// Persistent deterministic portable CPU packages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuArtifactStore {
    /// Packages by compiler-owned content identity.
    pub packages: BTreeMap<CpuArtifactId, CpuArtifactPackage>,
    /// Ordered publication history.
    pub events: Vec<VersionedCpuArtifactEvent>,
}

impl CpuArtifactStore {
    /// Atomically publishes one trusted lowering result.
    pub fn emit(
        &mut self,
        schedule_plan: SchedulePlanId,
        schedule_revision: ScheduleRevisionId,
        schedule_event_cursor: u64,
        draft: CpuArtifactDraft,
    ) -> AgentResult<CpuArtifactCheckReport> {
        let mut package = CpuArtifactPackage {
            id: CpuArtifactId::new("cpuart-pending"),
            format: "agentir.cpu.scalar.package".to_owned(),
            format_version: CPU_ARTIFACT_FORMAT_VERSION,
            target_profile: "cpu_scalar_v1".to_owned(),
            anchor: draft.anchor,
            compiler_build_hash: cpu_compiler_build_hash(),
            bindings: draft.bindings,
            constants: draft.constants,
            functions: draft.functions,
            outputs: draft.outputs,
            status: CpuArtifactStatus::Validated,
            cpu_artifact_hash: CpuArtifactHash::new("pending"),
            certificate: CpuArtifactCertificate {
                relation: "cpu_artifact_equivalent_to_schedule".to_owned(),
                schedule_hash: ScheduleHash::new("pending"),
                cpu_artifact_hash: CpuArtifactHash::new("pending"),
                conditions: CPU_ARTIFACT_CERTIFICATE_CONDITIONS
                    .map(str::to_owned)
                    .to_vec(),
                validator_version: CPU_ARTIFACT_VALIDATOR_VERSION,
            },
        };
        package.certificate.schedule_hash = package.anchor.schedule_hash.clone();
        package.cpu_artifact_hash = cpu_artifact_hash(&package)?;
        package.certificate.cpu_artifact_hash = package.cpu_artifact_hash.clone();
        package.id = CpuArtifactId::new(format!(
            "cpuart-{}",
            &package.cpu_artifact_hash.as_str()[..16]
        ));
        verify_cpu_artifact(&package)?;
        if let Some(existing) = self.packages.get(&package.id) {
            if existing == &package {
                return self.check(&package.id);
            }
            return Err(cpu_error(
                ErrorCode::CpuArtifactInvalid,
                "CPU artifact content identity collision",
            ));
        }
        self.packages.insert(package.id.clone(), package.clone());
        self.events.push(VersionedCpuArtifactEvent {
            semantics_version: CPU_ARTIFACT_EVENT_SEMANTICS_VERSION,
            event: CpuArtifactEvent {
                schedule_plan,
                schedule_revision,
                package: package.clone(),
                schedule_event_cursor,
            },
        });
        self.check(&package.id)
    }

    /// Returns deterministic summaries in content-ID order.
    pub fn list(&self) -> AgentResult<Vec<CpuArtifactQuery>> {
        self.packages.values().map(cpu_artifact_query).collect()
    }

    /// Returns one retained package.
    pub fn package(&self, artifact: &CpuArtifactId) -> AgentResult<&CpuArtifactPackage> {
        self.packages.get(artifact).ok_or_else(|| {
            cpu_error(
                ErrorCode::CpuArtifactNotFound,
                format!("CPU artifact `{artifact}` does not exist"),
            )
        })
    }

    /// Returns one deterministic summary.
    pub fn query(&self, artifact: &CpuArtifactId) -> AgentResult<CpuArtifactQuery> {
        cpu_artifact_query(self.package(artifact)?)
    }

    /// Fully validates one retained package without execution.
    pub fn check(&self, artifact: &CpuArtifactId) -> AgentResult<CpuArtifactCheckReport> {
        let package = self.package(artifact)?;
        verify_cpu_artifact(package)?;
        Ok(CpuArtifactCheckReport {
            query: cpu_artifact_query(package)?,
            offline_valid: true,
            equivalent_to_schedule: true,
        })
    }
}

fn cpu_artifact_query(package: &CpuArtifactPackage) -> AgentResult<CpuArtifactQuery> {
    Ok(CpuArtifactQuery {
        cpu_artifact: package.id.clone(),
        cpu_artifact_hash: package.cpu_artifact_hash.clone(),
        schedule_hash: package.anchor.schedule_hash.clone(),
        target_hash: package.anchor.target_hash.clone(),
        compiler_build_hash: package.compiler_build_hash.clone(),
        binding_count: package.bindings.len(),
        constant_count: package.constants.len(),
        instruction_count: package
            .functions
            .iter()
            .map(|function| function.instructions.len())
            .sum(),
        canonical_bytes: canonical_cpu_artifact_bytes(package)?.len(),
        status: package.status,
    })
}
