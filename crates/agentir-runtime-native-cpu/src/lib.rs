//! Safe bounded parent runtime for isolated Stage 9 native CPU execution.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    cpu::{
        CpuArtifactHash, CpuArtifactPackage, CpuCompilerBuildHash, CpuExtent, CpuInstruction,
        CpuValueType, verify_cpu_artifact,
    },
    cpu_measurement::{CpuInputHash, CpuOutputHash, cpu_input_hash, cpu_output_hash},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fmt::Write as _,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Version of the bounded internal worker request/response envelope.
pub const NATIVE_WORKER_PROTOCOL_VERSION: u32 = 1;
/// Version of the worker-local packed-buffer call ABI.
pub const NATIVE_CALL_ABI_VERSION: u32 = 1;
/// Exact pinned Cranelift family required by the parent.
pub const CRANELIFT_VERSION: &str = "0.116.1";
/// AgentIR native runtime build identity.
pub const NATIVE_RUNTIME_BUILD: &str =
    concat!("agentir_native_cpu_worker_", env!("CARGO_PKG_VERSION"));
/// Exact target triple of the safe parent executable build.
pub const PARENT_TARGET_TRIPLE: &str = env!("AGENTIR_NATIVE_PARENT_TARGET");
/// Execution observation format version.
pub const CPU_NATIVE_EXECUTION_FORMAT_VERSION: u32 = 1;
/// Domain separator for the native runtime observation identity.
pub const CPU_NATIVE_RUNTIME_HASH_DOMAIN: &[u8] = b"agentir.cpu.native.runtime.v1\0";
/// Domain separator for complete native execution observations.
pub const CPU_NATIVE_EXECUTION_HASH_DOMAIN: &[u8] = b"agentir.cpu.native.execution.v1\0";
/// Maximum encoded request or response accepted by the internal worker.
pub const MAX_WORKER_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// Hidden process mode selected by the server before JSONL processing.
pub const HIDDEN_WORKER_ARGUMENT: &str = "--agentir-native-cpu-worker-v1";
/// Fixed server-owned Cranelift settings, in canonical order.
pub const FIXED_CODEGEN_SETTINGS: [(&str, &str); 4] = [
    ("enable_verifier", "true"),
    ("is_pic", "false"),
    ("opt_level", "none"),
    ("use_colocated_libcalls", "false"),
];

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// One bounded internal request. This type is never exposed by JSONL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkerRequest {
    /// Internal protocol version.
    pub protocol_version: u32,
    /// Unchanged compiler-published Stage 8A package.
    pub package: CpuArtifactPackage,
    /// Exact retained package hash expected by the parent.
    pub expected_cpu_artifact_hash: CpuArtifactHash,
    /// Ordinary runtime inputs using the unchanged Stage 8A binding interface.
    pub inputs: BTreeMap<String, Value>,
}

/// Observation-only identity of the worker code-generation runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuNativeRuntimeIdentity {
    /// Internal worker protocol version.
    pub worker_protocol_version: u32,
    /// AgentIR native worker build identity.
    pub runtime_build: String,
    /// Exact pinned Cranelift family.
    pub cranelift_version: String,
    /// Server-detected host target triple.
    pub target_triple: String,
    /// ISA features actually enabled by host detection, canonically ordered.
    pub enabled_isa_features: Vec<String>,
    /// Fixed server-owned code-generation settings, canonically ordered.
    pub codegen_settings: Vec<String>,
    /// Internal packed-buffer ABI version.
    pub abi_version: u32,
}

/// Checked resource projection performed before a worker is launched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuNativeProjectedWork {
    /// Tensor buffers retained by bindings and instruction results.
    pub buffers: u64,
    /// Scalar elements retained by bindings and instruction results.
    pub elements: u64,
    /// Bytes required by all retained f32 values.
    pub bytes: u64,
    /// Top-level plus scalar-body instruction work.
    pub instructions: u64,
}

/// Canonical output shape entry in compiler-owned output order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuNativeOutputShape {
    /// Compiler-owned output name.
    pub name: String,
    /// Scalar or one-dimensional tensor value type.
    pub value_type: CpuValueType,
    /// Exact runtime extent; absent for a scalar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<u64>,
}

/// Identity fields independently recomputed on both sides of the process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuNativeExecutionIdentity {
    /// Canonical ordinary input identity reused from Stage 8B.
    pub cpu_input_hash: CpuInputHash,
    /// Exact checked work projection.
    pub projected_work: CpuNativeProjectedWork,
    /// Ordered output type/shape model.
    pub output_shapes: Vec<CpuNativeOutputShape>,
    /// Deterministic output anchor reused from Stage 8B.
    pub output_hash: CpuOutputHash,
    /// Complete observation-only native execution identity.
    pub cpu_native_execution_hash: String,
}

/// Successful bounded internal native execution response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkerSuccess {
    /// Internal protocol version.
    pub protocol_version: u32,
    /// Observation-only native runtime identity.
    pub runtime: CpuNativeRuntimeIdentity,
    /// Runtime identity hash claimed by the worker.
    pub cpu_native_runtime_hash: String,
    /// Exact Stage 8A package consumed by the worker.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Named finite outputs produced by one native call.
    pub outputs: BTreeMap<String, Value>,
    /// Worker-computed execution identity for independent parent validation.
    pub execution: CpuNativeExecutionIdentity,
}

/// One typed internal worker failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkerFailure {
    /// Stable internal diagnostic label.
    pub code: String,
    /// Human-readable failure description.
    pub message: String,
}

/// Exactly one bounded response emitted before worker exit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeWorkerResponse {
    /// Native lowering and one call completed.
    Ok(Box<NativeWorkerSuccess>),
    /// Validation, lowering, process, or response validation failed.
    Error {
        /// Typed internal failure.
        error: NativeWorkerFailure,
    },
}

/// Internal worker implementation error, converted to a bounded wire rejection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeWorkerError {
    code: String,
    message: String,
}

impl NativeWorkerError {
    /// Creates one internal worker error.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Creates a typed error for the one-request worker mode.
    #[must_use]
    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    /// Wraps worker I/O; this message is never a public semantic diagnostic.
    #[must_use]
    pub fn from_io(code: impl Into<String>, error: &std::io::Error) -> Self {
        Self::new(code, error.to_string())
    }
}

impl fmt::Display for NativeWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NativeWorkerError {}

/// Internal result used by the worker implementation and Stage 9A tests.
pub type NativeWorkerResult<T> = Result<T, NativeWorkerError>;

/// Encodes a typed failure response.
#[must_use]
pub fn failure_response(error: &NativeWorkerError) -> NativeWorkerResponse {
    NativeWorkerResponse::Error {
        error: NativeWorkerFailure {
            code: error.code.clone(),
            message: error.message.clone(),
        },
    }
}

#[derive(Serialize)]
struct RuntimeHashModel<'a> {
    worker_protocol_version: u32,
    runtime_build: &'a str,
    cranelift_version: &'a str,
    target_triple: &'a str,
    enabled_isa_features: &'a [String],
    codegen_settings: &'a [String],
    abi_version: u32,
}

#[derive(Serialize)]
struct ExecutionHashModel<'a> {
    format_version: u32,
    cpu_artifact_hash: &'a CpuArtifactHash,
    compiler_build_hash: &'a CpuCompilerBuildHash,
    cpu_native_runtime_hash: &'a str,
    cpu_input_hash: &'a CpuInputHash,
    projected_work: &'a CpuNativeProjectedWork,
    output_shapes: &'a [CpuNativeOutputShape],
    output_hash: &'a CpuOutputHash,
}

fn digest(domain: &[u8], value: &impl Serialize, code: ErrorCode) -> AgentResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| AgentError::new(code, "native identity encoding failed"))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    let mut result = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(result, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(result)
}

/// Computes the canonical native runtime hash from its visible identity fields.
pub fn cpu_native_runtime_hash(identity: &CpuNativeRuntimeIdentity) -> AgentResult<String> {
    digest(
        CPU_NATIVE_RUNTIME_HASH_DOMAIN,
        &RuntimeHashModel {
            worker_protocol_version: identity.worker_protocol_version,
            runtime_build: &identity.runtime_build,
            cranelift_version: &identity.cranelift_version,
            target_triple: &identity.target_triple,
            enabled_isa_features: &identity.enabled_isa_features,
            codegen_settings: &identity.codegen_settings,
            abi_version: identity.abi_version,
        },
        ErrorCode::CpuNativeRuntimeIdentityMismatch,
    )
}

/// Computes the complete observation-only native execution hash.
pub fn cpu_native_execution_hash(
    package: &CpuArtifactPackage,
    runtime_hash: &str,
    input_hash: &CpuInputHash,
    projected_work: &CpuNativeProjectedWork,
    output_shapes: &[CpuNativeOutputShape],
    output_hash: &CpuOutputHash,
) -> AgentResult<String> {
    digest(
        CPU_NATIVE_EXECUTION_HASH_DOMAIN,
        &ExecutionHashModel {
            format_version: CPU_NATIVE_EXECUTION_FORMAT_VERSION,
            cpu_artifact_hash: &package.cpu_artifact_hash,
            compiler_build_hash: &package.compiler_build_hash,
            cpu_native_runtime_hash: runtime_hash,
            cpu_input_hash: input_hash,
            projected_work,
            output_shapes,
            output_hash,
        },
        ErrorCode::CpuNativeExecutionIdentityMismatch,
    )
}

/// Fully checked parent-side request preparation, reusable by the independently validating worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedNativeExecution {
    /// Runtime bindings for symbolic extents.
    pub dimensions: BTreeMap<String, u64>,
    /// Exact checked work projection.
    pub projected_work: CpuNativeProjectedWork,
    /// Expected ordered output shapes.
    pub output_shapes: Vec<CpuNativeOutputShape>,
    /// Canonical input hash.
    pub cpu_input_hash: CpuInputHash,
}

fn native_validation(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::CpuNativeValidationFailed, message)
}

fn finite_f32(value: &Value, context: &str) -> AgentResult<f32> {
    let number = value
        .as_f64()
        .ok_or_else(|| native_validation(format!("{context} is not numeric")))?;
    let converted = number as f32;
    if !converted.is_finite() {
        return Err(native_validation(format!(
            "{context} is outside finite f32"
        )));
    }
    Ok(converted)
}

fn resolve_extent(extent: &CpuExtent, dimensions: &BTreeMap<String, u64>) -> AgentResult<u64> {
    match extent {
        CpuExtent::Static { value } => Ok(*value),
        CpuExtent::Symbol { name } => dimensions
            .get(name)
            .copied()
            .ok_or_else(|| native_validation(format!("runtime dimension `{name}` is unbound"))),
    }
}

/// Verifies the retained package, inputs, shapes, checked projections, and resource policy.
pub fn prepare_native_execution(
    package: &CpuArtifactPackage,
    expected_hash: &CpuArtifactHash,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
) -> AgentResult<PreparedNativeExecution> {
    verify_cpu_artifact(package).map_err(|error| {
        native_validation("retained Stage 8A package failed structural validation")
            .with_detail("cause_code", json!(error.code))
    })?;
    if &package.cpu_artifact_hash != expected_hash {
        return Err(AgentError::new(
            ErrorCode::CpuNativeArtifactIdentityMismatch,
            "expected CPU artifact hash differs from the retained package",
        )
        .with_types(json!(package.cpu_artifact_hash), json!(expected_hash)));
    }
    let expected_names = package
        .bindings
        .iter()
        .map(|binding| binding.name.clone())
        .collect::<BTreeSet<_>>();
    let actual_names = inputs.keys().cloned().collect::<BTreeSet<_>>();
    if expected_names != actual_names {
        return Err(
            native_validation("native runtime input names differ from the package ABI")
                .with_detail("expected_names", json!(expected_names))
                .with_detail("actual_names", json!(actual_names)),
        );
    }
    let mut dimensions = BTreeMap::new();
    let mut elements = 0_u64;
    let mut buffers = 0_u64;
    for binding in &package.bindings {
        let input = &inputs[&binding.name];
        match binding.value_type {
            CpuValueType::F32 => {
                finite_f32(input, &format!("input `{}`", binding.name))?;
                elements = elements
                    .checked_add(1)
                    .ok_or_else(|| native_validation("native input element projection overflow"))?;
            }
            CpuValueType::F32Tensor1d => {
                let items = input.as_array().ok_or_else(|| {
                    native_validation(format!(
                        "input `{}` must be a one-dimensional f32 tensor",
                        binding.name
                    ))
                })?;
                for (index, item) in items.iter().enumerate() {
                    finite_f32(item, &format!("input `{}` element {index}", binding.name))?;
                }
                let length = u64::try_from(items.len())
                    .map_err(|_| native_validation("native tensor extent exceeds u64"))?;
                match binding
                    .extent
                    .as_ref()
                    .ok_or_else(|| native_validation("tensor binding lacks an extent"))?
                {
                    CpuExtent::Static { value } if *value == length => {}
                    CpuExtent::Static { value } => {
                        return Err(native_validation(format!(
                            "input `{}` has extent {length}, expected {value}",
                            binding.name
                        )));
                    }
                    CpuExtent::Symbol { name } => {
                        if dimensions
                            .insert(name.clone(), length)
                            .is_some_and(|old| old != length)
                        {
                            return Err(native_validation(format!(
                                "runtime dimension `{name}` is inconsistent"
                            )));
                        }
                    }
                }
                elements = elements
                    .checked_add(length)
                    .ok_or_else(|| native_validation("native input element projection overflow"))?;
                buffers = buffers
                    .checked_add(1)
                    .ok_or_else(|| native_validation("native buffer projection overflow"))?;
            }
        }
    }
    let function = package
        .functions
        .first()
        .ok_or_else(|| native_validation("CPU package has no entry function"))?;
    let mut instructions = u64::try_from(function.instructions.len())
        .map_err(|_| native_validation("native instruction count exceeds u64"))?;
    let mut register_extents = BTreeMap::new();
    for binding in &package.bindings {
        let extent = match binding.value_type {
            CpuValueType::F32 => None,
            CpuValueType::F32Tensor1d => Some(resolve_extent(
                binding
                    .extent
                    .as_ref()
                    .ok_or_else(|| native_validation("tensor binding lacks an extent"))?,
                &dimensions,
            )?),
        };
        register_extents.insert(binding.register, extent);
    }
    for instruction in &function.instructions {
        let extent = match instruction {
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. }
            | CpuInstruction::FmaF32 { .. } => {
                elements = elements
                    .checked_add(1)
                    .ok_or_else(|| native_validation("native element projection overflow"))?;
                None
            }
            CpuInstruction::MapF32 { extent, body, .. }
            | CpuInstruction::ZipMapF32 { extent, body, .. } => {
                let length = resolve_extent(extent, &dimensions)?;
                elements = elements
                    .checked_add(length)
                    .ok_or_else(|| native_validation("native element projection overflow"))?;
                buffers = buffers
                    .checked_add(1)
                    .ok_or_else(|| native_validation("native buffer projection overflow"))?;
                instructions = instructions
                    .checked_add(
                        length
                            .checked_mul(u64::try_from(body.instructions.len()).map_err(|_| {
                                native_validation("native scalar body count exceeds u64")
                            })?)
                            .ok_or_else(|| native_validation("native instruction work overflow"))?,
                    )
                    .ok_or_else(|| native_validation("native instruction work overflow"))?;
                Some(length)
            }
        };
        register_extents.insert(instruction.output(), extent);
    }
    let bytes = elements
        .checked_mul(u64::try_from(std::mem::size_of::<f32>()).unwrap_or(4))
        .ok_or_else(|| native_validation("native byte projection overflow"))?;
    for (kind, actual, context) in [
        (
            ResourceKind::ExecutionBuffers,
            buffers,
            "native CPU buffers",
        ),
        (
            ResourceKind::ExecutionElements,
            elements,
            "native CPU retained elements",
        ),
        (
            ResourceKind::ExecutionBytes,
            bytes,
            "native CPU retained bytes",
        ),
        (
            ResourceKind::ExecutionElements,
            instructions,
            "native CPU instruction work",
        ),
    ] {
        BudgetCheck::against(limits, kind, actual, context).map_err(|error| {
            native_validation("native CPU resource projection exceeds server policy")
                .with_detail("cause_code", json!(error.code))
                .with_detail("resource", json!(kind))
                .with_detail("actual", json!(actual))
        })?;
    }
    let output_shapes = package
        .outputs
        .iter()
        .map(|output| {
            let extent = register_extents.get(&output.register).copied().flatten();
            CpuNativeOutputShape {
                name: output.name.clone(),
                value_type: output.value_type,
                extent,
            }
        })
        .collect();
    Ok(PreparedNativeExecution {
        dimensions,
        projected_work: CpuNativeProjectedWork {
            buffers,
            elements,
            bytes,
            instructions,
        },
        output_shapes,
        cpu_input_hash: cpu_input_hash(inputs)?,
    })
}

fn canonical_outputs(
    outputs: &BTreeMap<String, Value>,
    shapes: &[CpuNativeOutputShape],
) -> AgentResult<BTreeMap<String, Value>> {
    let expected = shapes
        .iter()
        .map(|shape| shape.name.clone())
        .collect::<BTreeSet<_>>();
    let actual = outputs.keys().cloned().collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(AgentError::new(
            ErrorCode::CpuNativeOutputInvalid,
            "native worker output names are incomplete or contain extras",
        )
        .with_detail("expected_names", json!(expected))
        .with_detail("actual_names", json!(actual)));
    }
    let mut canonical = BTreeMap::new();
    for shape in shapes {
        let raw = &outputs[&shape.name];
        let value = match shape.value_type {
            CpuValueType::F32 => {
                if shape.extent.is_some() {
                    return Err(AgentError::new(
                        ErrorCode::CpuNativeOutputInvalid,
                        "scalar native output has a tensor extent",
                    ));
                }
                let value = finite_f32(raw, &format!("output `{}`", shape.name)).map_err(|_| {
                    AgentError::new(
                        ErrorCode::CpuNativeOutputInvalid,
                        "native scalar output is not finite f32",
                    )
                })?;
                Value::Number(
                    Number::from_f64(f64::from(value)).expect("finite f32 encodes as JSON"),
                )
            }
            CpuValueType::F32Tensor1d => {
                let items = raw.as_array().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::CpuNativeOutputInvalid,
                        "native tensor output is not an array",
                    )
                })?;
                if shape.extent != Some(u64::try_from(items.len()).unwrap_or(u64::MAX)) {
                    return Err(AgentError::new(
                        ErrorCode::CpuNativeOutputInvalid,
                        "native tensor output has the wrong extent",
                    )
                    .with_types(json!(shape.extent), json!(items.len())));
                }
                Value::Array(
                    items
                        .iter()
                        .map(|item| {
                            let value = finite_f32(item, "native tensor output").map_err(|_| {
                                AgentError::new(
                                    ErrorCode::CpuNativeOutputInvalid,
                                    "native tensor output contains a non-finite f32",
                                )
                            })?;
                            Ok(Value::Number(
                                Number::from_f64(f64::from(value))
                                    .expect("finite f32 encodes as JSON"),
                            ))
                        })
                        .collect::<AgentResult<Vec<_>>>()?,
                )
            }
        };
        canonical.insert(shape.name.clone(), value);
    }
    Ok(canonical)
}

/// Builds the execution identity from already validated inputs and outputs.
pub fn build_execution_identity(
    package: &CpuArtifactPackage,
    runtime_hash: &str,
    prepared: &PreparedNativeExecution,
    outputs: &BTreeMap<String, Value>,
) -> AgentResult<(BTreeMap<String, Value>, CpuNativeExecutionIdentity)> {
    let outputs = canonical_outputs(outputs, &prepared.output_shapes)?;
    let output_hash = cpu_output_hash(&outputs)?;
    let execution_hash = cpu_native_execution_hash(
        package,
        runtime_hash,
        &prepared.cpu_input_hash,
        &prepared.projected_work,
        &prepared.output_shapes,
        &output_hash,
    )?;
    Ok((
        outputs,
        CpuNativeExecutionIdentity {
            cpu_input_hash: prepared.cpu_input_hash.clone(),
            projected_work: prepared.projected_work.clone(),
            output_shapes: prepared.output_shapes.clone(),
            output_hash,
            cpu_native_execution_hash: execution_hash,
        },
    ))
}

/// Final public observation returned by `cpu_native.execute`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuNativeExecutionObservation {
    /// Named canonical finite outputs.
    pub outputs: BTreeMap<String, Value>,
    /// Exact retained Stage 8A artifact hash.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Stage 8A CPU compiler build identity.
    pub compiler_build_hash: CpuCompilerBuildHash,
    /// Canonical ordinary input identity.
    pub cpu_input_hash: CpuInputHash,
    /// Runtime-owned identity fields.
    pub runtime: CpuNativeRuntimeIdentity,
    /// Independent runtime identity hash.
    pub cpu_native_runtime_hash: String,
    /// Checked resource/work projection.
    pub projected_work: CpuNativeProjectedWork,
    /// Ordered output shape model.
    pub output_shapes: Vec<CpuNativeOutputShape>,
    /// Deterministic output anchor.
    pub output_hash: CpuOutputHash,
    /// Complete observation-only execution identity.
    pub cpu_native_execution_hash: String,
}

/// Narrow injectable worker-launch boundary shared by production and explicit tests.
pub trait NativeWorkerLauncher: Send {
    /// Starts exactly one fresh worker attempt and returns its one response.
    fn launch(
        &mut self,
        request: &NativeWorkerRequest,
        timeout: Duration,
    ) -> AgentResult<NativeWorkerSuccess>;
}

/// Server-owned native execution policy. It is never serialized or hashed.
#[derive(Clone, Debug)]
pub struct NativeCpuPolicy {
    timeout: Duration,
}

impl Default for NativeCpuPolicy {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl NativeCpuPolicy {
    /// Creates a bounded policy for explicit tests; no public request can reach it.
    #[doc(hidden)]
    #[must_use]
    pub const fn for_tests(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// Production launcher using a hidden mode of a server-owned executable.
#[derive(Clone, Debug)]
pub struct ProcessNativeWorkerLauncher {
    executable: Option<PathBuf>,
    hidden_mode: bool,
}

impl Default for ProcessNativeWorkerLauncher {
    fn default() -> Self {
        Self {
            executable: None,
            hidden_mode: true,
        }
    }
}

impl ProcessNativeWorkerLauncher {
    /// Uses the current server executable and its hidden pre-JSONL worker mode.
    pub fn current_executable() -> AgentResult<Self> {
        std::env::current_exe()
            .map(|executable| Self {
                executable: Some(executable),
                hidden_mode: true,
            })
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::CpuNativeWorkerUnavailable,
                    "native worker executable is unavailable",
                )
            })
    }

    /// Uses a dedicated server-owned worker executable. Intended for worker integration tests.
    #[doc(hidden)]
    #[must_use]
    pub fn dedicated(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
            hidden_mode: false,
        }
    }
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn bounded_reader<R: Read + Send + 'static>(
    reader: R,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .take(u64::try_from(MAX_WORKER_FRAME_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn read_join(handle: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> AgentResult<Vec<u8>> {
    handle
        .join()
        .map_err(|_| {
            AgentError::new(
                ErrorCode::CpuNativeWorkerCrashed,
                "native worker output reader failed",
            )
        })?
        .map_err(|_| {
            AgentError::new(
                ErrorCode::CpuNativeWorkerResponseMalformed,
                "native worker output could not be read",
            )
        })
}

fn check_exit(status: ExitStatus) -> AgentResult<()> {
    if status.success() {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::CpuNativeWorkerCrashed,
            "native worker exited unsuccessfully",
        ))
    }
}

fn launch_process_once(
    executable: &Path,
    hidden_mode: bool,
    request: &NativeWorkerRequest,
    timeout: Duration,
) -> AgentResult<NativeWorkerSuccess> {
    let encoded = serde_json::to_vec(request).map_err(|_| {
        AgentError::new(
            ErrorCode::CpuNativeValidationFailed,
            "native worker request encoding failed",
        )
    })?;
    if encoded.len() > MAX_WORKER_FRAME_BYTES {
        return Err(AgentError::new(
            ErrorCode::CpuNativeValidationFailed,
            "native worker request exceeds the internal frame limit",
        ));
    }
    let mut command = Command::new(executable);
    if hidden_mode {
        command.arg(HIDDEN_WORKER_ARGUMENT);
    }
    let mut child = command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            AgentError::new(
                ErrorCode::CpuNativeWorkerUnavailable,
                "native worker could not be started",
            )
        })?;
    let Some(stdin) = child.stdin.take() else {
        terminate_and_reap(&mut child);
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerUnavailable,
            "native worker stdin is unavailable",
        ));
    };
    let Some(stdout) = child.stdout.take() else {
        terminate_and_reap(&mut child);
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerUnavailable,
            "native worker stdout is unavailable",
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_and_reap(&mut child);
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerUnavailable,
            "native worker stderr is unavailable",
        ));
    };
    let stdout_reader = bounded_reader(stdout);
    let stderr_reader = bounded_reader(stderr);
    let started = Instant::now();
    let stdin_writer = thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(&encoded)
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                terminate_and_reap(&mut child);
                let _ = stdin_writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(AgentError::new(
                    ErrorCode::CpuNativeWorkerTimeout,
                    "native worker exceeded the server-owned timeout",
                ));
            }
            Err(_) => {
                terminate_and_reap(&mut child);
                let _ = stdin_writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(AgentError::new(
                    ErrorCode::CpuNativeWorkerCrashed,
                    "native worker status could not be read",
                ));
            }
        }
    };
    let write_result = stdin_writer.join().map_err(|_| {
        AgentError::new(
            ErrorCode::CpuNativeWorkerCrashed,
            "native worker input writer failed",
        )
    })?;
    if write_result.is_err() {
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerCrashed,
            "native worker rejected its request stream",
        ));
    }
    let stdout = read_join(stdout_reader)?;
    let stderr = read_join(stderr_reader)?;
    check_exit(status)?;
    if !stderr.is_empty() {
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerResponseMalformed,
            "native worker emitted stderr",
        ));
    }
    if stdout.len() > MAX_WORKER_FRAME_BYTES {
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerResponseMalformed,
            "native worker response exceeds the internal frame limit",
        ));
    }
    let response: NativeWorkerResponse = serde_json::from_slice(&stdout).map_err(|_| {
        AgentError::new(
            ErrorCode::CpuNativeWorkerResponseMalformed,
            "native worker response is malformed or truncated",
        )
    })?;
    let canonical = serde_json::to_vec(&response).map_err(|_| {
        AgentError::new(
            ErrorCode::CpuNativeWorkerResponseMalformed,
            "native worker response cannot be canonicalized",
        )
    })?;
    if stdout != canonical {
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerResponseMalformed,
            "native worker response contains trailing or non-canonical data",
        ));
    }
    match response {
        NativeWorkerResponse::Ok(success) => Ok(*success),
        NativeWorkerResponse::Error { error } => Err(AgentError::new(
            ErrorCode::CpuNativeWorkerRejected,
            "native worker rejected the execution",
        )
        .with_detail("worker_code", json!(error.code))),
    }
}

impl NativeWorkerLauncher for ProcessNativeWorkerLauncher {
    fn launch(
        &mut self,
        request: &NativeWorkerRequest,
        timeout: Duration,
    ) -> AgentResult<NativeWorkerSuccess> {
        let executable = match &self.executable {
            Some(executable) => executable.clone(),
            None => std::env::current_exe().map_err(|_| {
                AgentError::new(
                    ErrorCode::CpuNativeWorkerUnavailable,
                    "native worker executable is unavailable",
                )
            })?,
        };
        launch_process_once(&executable, self.hidden_mode, request, timeout)
    }
}

/// Compatibility launcher for Stage 9A dedicated-worker tests.
pub fn launch_worker_once(
    worker_executable: &Path,
    request: &NativeWorkerRequest,
) -> NativeWorkerResult<NativeWorkerSuccess> {
    launch_process_once(worker_executable, false, request, DEFAULT_TIMEOUT).map_err(|error| {
        let code = if error.code == ErrorCode::CpuNativeWorkerRejected {
            error
                .details
                .get("worker_code")
                .and_then(Value::as_str)
                .unwrap_or("WORKER_REJECTED")
                .to_owned()
        } else {
            match error.code {
                ErrorCode::CpuNativeWorkerResponseMalformed => "WORKER_RESPONSE_MALFORMED",
                ErrorCode::CpuNativeWorkerTimeout => "WORKER_TIMEOUT",
                ErrorCode::CpuNativeWorkerUnavailable => "WORKER_SPAWN",
                _ => "WORKER_EXIT",
            }
            .to_owned()
        };
        NativeWorkerError::new(code, error.to_string())
    })
}

fn validate_runtime(
    identity: &CpuNativeRuntimeIdentity,
    claimed_hash: &str,
) -> AgentResult<String> {
    let mut features = identity.enabled_isa_features.clone();
    features.sort();
    features.dedup();
    let expected_settings = FIXED_CODEGEN_SETTINGS
        .map(|(name, value)| format!("{name}={value}"))
        .to_vec();
    let features_are_canonical = identity.enabled_isa_features.iter().all(|feature| {
        feature
            .split_once('=')
            .is_some_and(|(name, value)| !name.is_empty() && !value.is_empty())
    });
    if identity.worker_protocol_version != NATIVE_WORKER_PROTOCOL_VERSION
        || identity.runtime_build != NATIVE_RUNTIME_BUILD
        || identity.cranelift_version != CRANELIFT_VERSION
        || identity.target_triple != PARENT_TARGET_TRIPLE
        || identity.enabled_isa_features != features
        || !features_are_canonical
        || identity.codegen_settings != expected_settings
        || identity.abi_version != NATIVE_CALL_ABI_VERSION
    {
        return Err(AgentError::new(
            ErrorCode::CpuNativeRuntimeIdentityMismatch,
            "native worker runtime identity is incompatible",
        ));
    }
    let expected = cpu_native_runtime_hash(identity)?;
    if expected != claimed_hash {
        return Err(AgentError::new(
            ErrorCode::CpuNativeRuntimeIdentityMismatch,
            "native worker runtime hash is inconsistent",
        )
        .with_types(json!(expected), json!(claimed_hash)));
    }
    Ok(expected)
}

/// Executes one retained package through exactly one injected worker launch.
pub fn execute_with_launcher(
    package: &CpuArtifactPackage,
    expected_hash: &CpuArtifactHash,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
    policy: &NativeCpuPolicy,
    launcher: &mut dyn NativeWorkerLauncher,
) -> AgentResult<CpuNativeExecutionObservation> {
    let prepared = prepare_native_execution(package, expected_hash, inputs, limits)?;
    let request = NativeWorkerRequest {
        protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
        package: package.clone(),
        expected_cpu_artifact_hash: expected_hash.clone(),
        inputs: inputs.clone(),
    };
    let response = launcher.launch(&request, policy.timeout)?;
    if response.protocol_version != NATIVE_WORKER_PROTOCOL_VERSION {
        return Err(AgentError::new(
            ErrorCode::CpuNativeWorkerProtocolMismatch,
            "native worker protocol version is stale",
        )
        .with_types(
            json!(NATIVE_WORKER_PROTOCOL_VERSION),
            json!(response.protocol_version),
        ));
    }
    if response.cpu_artifact_hash != package.cpu_artifact_hash {
        return Err(AgentError::new(
            ErrorCode::CpuNativeArtifactIdentityMismatch,
            "native worker reported the wrong artifact identity",
        ));
    }
    let runtime_hash = validate_runtime(&response.runtime, &response.cpu_native_runtime_hash)?;
    let (outputs, execution) =
        build_execution_identity(package, &runtime_hash, &prepared, &response.outputs)?;
    if execution != response.execution {
        return Err(AgentError::new(
            ErrorCode::CpuNativeExecutionIdentityMismatch,
            "native worker execution identity is inconsistent",
        )
        .with_types(json!(execution), json!(response.execution)));
    }
    Ok(CpuNativeExecutionObservation {
        outputs,
        cpu_artifact_hash: package.cpu_artifact_hash.clone(),
        compiler_build_hash: package.compiler_build_hash.clone(),
        cpu_input_hash: execution.cpu_input_hash,
        runtime: response.runtime,
        cpu_native_runtime_hash: runtime_hash,
        projected_work: execution.projected_work,
        output_shapes: execution.output_shapes,
        output_hash: execution.output_hash,
        cpu_native_execution_hash: execution.cpu_native_execution_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CpuNativeRuntimeIdentity {
        CpuNativeRuntimeIdentity {
            worker_protocol_version: NATIVE_WORKER_PROTOCOL_VERSION,
            runtime_build: NATIVE_RUNTIME_BUILD.to_owned(),
            cranelift_version: CRANELIFT_VERSION.to_owned(),
            target_triple: "test-target".to_owned(),
            enabled_isa_features: vec!["feature=true".to_owned()],
            codegen_settings: FIXED_CODEGEN_SETTINGS
                .map(|(name, value)| format!("{name}={value}"))
                .to_vec(),
            abi_version: NATIVE_CALL_ABI_VERSION,
        }
    }

    #[test]
    fn runtime_hash_covers_every_runtime_identity_field() {
        let baseline = identity();
        let baseline_hash = cpu_native_runtime_hash(&baseline).unwrap();
        let variants = [
            CpuNativeRuntimeIdentity {
                worker_protocol_version: 2,
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                runtime_build: "other".to_owned(),
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                cranelift_version: "other".to_owned(),
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                target_triple: "other".to_owned(),
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                enabled_isa_features: Vec::new(),
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                codegen_settings: Vec::new(),
                ..baseline.clone()
            },
            CpuNativeRuntimeIdentity {
                abi_version: 2,
                ..baseline
            },
        ];
        for variant in variants {
            assert_ne!(cpu_native_runtime_hash(&variant).unwrap(), baseline_hash);
        }
    }

    #[test]
    fn timeout_policy_is_private_and_absent_from_wire_identity() {
        let short = NativeCpuPolicy::for_tests(Duration::from_millis(1));
        let long = NativeCpuPolicy::for_tests(Duration::from_secs(1));
        assert_ne!(short.timeout, long.timeout);
        assert_eq!(
            cpu_native_runtime_hash(&identity()).unwrap(),
            cpu_native_runtime_hash(&identity()).unwrap()
        );
        let encoded = serde_json::to_value(identity()).unwrap();
        assert!(encoded.get("timeout").is_none());
        assert!(encoded.get("resource_limits").is_none());
    }

    #[test]
    fn internal_response_rejects_unknown_fields() {
        let response = json!({
            "status":"error",
            "error":{"code":"fixture","message":"fixture"},
            "extra":true
        });
        assert!(serde_json::from_value::<NativeWorkerResponse>(response).is_err());
    }
}
