//! Bounded Stage 8B orchestration around unchanged Stage 8A CPU packages.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    cpu::{CpuArtifactPackage, CpuExtent, CpuInstruction, CpuValueType},
    cpu_measurement::{
        CpuBenchmarkConfig, CpuClockSource, CpuHostDescriptor, CpuMeasurementDraft,
        aggregate_cpu_durations, validate_cpu_benchmark_config,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

/// Stage 8B runtime contract version retained in every measurement record.
pub const CPU_MEASUREMENT_RUNTIME_VERSION: &str = "agentir_runtime_cpu_v1";

/// Injectable monotonic nanosecond clock. Production protocol uses only `MonotonicClock`.
pub trait CpuClock {
    /// Returns the clock provenance label.
    fn source(&self) -> CpuClockSource;
    /// Returns a monotonic process-local nanosecond reading.
    fn now_ns(&mut self) -> AgentResult<u64>;
}

/// Explicit execution double used only by Stage 8 closure tests and fixtures.
///
/// Production acquisition does not accept this interface and always invokes the
/// unchanged Stage 8A interpreter directly.
#[doc(hidden)]
pub trait CpuExecutionTestDouble {
    /// Executes one structurally verified package while recording fixture-owned calls.
    fn execute(
        &mut self,
        package: &CpuArtifactPackage,
        inputs: &BTreeMap<String, Value>,
        limits: &ResourceLimits,
    ) -> AgentResult<agentir_backend_cpu::CpuExecutionResult>;
}

/// Production process-local monotonic clock.
#[derive(Debug)]
pub struct MonotonicClock {
    origin: Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl CpuClock for MonotonicClock {
    fn source(&self) -> CpuClockSource {
        CpuClockSource::ProductionMonotonicV1
    }

    fn now_ns(&mut self) -> AgentResult<u64> {
        u64::try_from(self.origin.elapsed().as_nanos()).map_err(|_| {
            AgentError::new(
                ErrorCode::CpuMeasurementOverflow,
                "monotonic clock reading exceeds u64 nanoseconds",
            )
        })
    }
}

fn host_descriptor(source: CpuClockSource) -> CpuHostDescriptor {
    CpuHostDescriptor {
        format_version: 1,
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        family: std::env::consts::FAMILY.to_owned(),
        runtime_version: CPU_MEASUREMENT_RUNTIME_VERSION.to_owned(),
        clock_source: source,
    }
}

fn tensor_dimensions(
    package: &CpuArtifactPackage,
    inputs: &BTreeMap<String, Value>,
) -> AgentResult<(BTreeMap<String, usize>, u64)> {
    let expected = package
        .bindings
        .iter()
        .map(|binding| binding.name.clone())
        .collect::<BTreeSet<_>>();
    let actual = inputs.keys().cloned().collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(AgentError::new(
            ErrorCode::CpuExecutionInputMismatch,
            "CPU measurement input names differ from the artifact ABI",
        ));
    }
    let mut dimensions = BTreeMap::new();
    let mut elements = 0_u64;
    for binding in &package.bindings {
        let input = &inputs[&binding.name];
        match binding.value_type {
            CpuValueType::F32 => {
                if input.as_f64().is_none() {
                    return Err(AgentError::new(
                        ErrorCode::CpuExecutionInputMismatch,
                        "CPU measurement scalar input is not numeric",
                    ));
                }
                elements = elements.checked_add(1).ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::CpuMeasurementOverflow,
                        "CPU measurement input element count overflow",
                    )
                })?;
            }
            CpuValueType::F32Tensor1d => {
                let values = input.as_array().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::CpuExecutionInputMismatch,
                        "CPU measurement tensor input is not an array",
                    )
                })?;
                if values.iter().any(|value| value.as_f64().is_none()) {
                    return Err(AgentError::new(
                        ErrorCode::CpuExecutionInputMismatch,
                        "CPU measurement tensor contains a non-number",
                    ));
                }
                let length = values.len();
                match binding.extent.as_ref() {
                    Some(CpuExtent::Static { value })
                        if usize::try_from(*value).ok() == Some(length) => {}
                    Some(CpuExtent::Symbol { name }) => {
                        if dimensions
                            .insert(name.clone(), length)
                            .is_some_and(|old| old != length)
                        {
                            return Err(AgentError::new(
                                ErrorCode::CpuExecutionInputMismatch,
                                "CPU measurement symbolic dimensions disagree",
                            ));
                        }
                    }
                    _ => {
                        return Err(AgentError::new(
                            ErrorCode::CpuExecutionInputMismatch,
                            "CPU measurement tensor extent is incompatible",
                        ));
                    }
                }
                elements = elements
                    .checked_add(u64::try_from(length).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::CpuMeasurementOverflow,
                            "CPU measurement input element count overflow",
                        )
                    })?;
            }
        }
    }
    Ok((dimensions, elements))
}

fn resolved_extent(extent: &CpuExtent, dimensions: &BTreeMap<String, usize>) -> AgentResult<u64> {
    match extent {
        CpuExtent::Static { value } => Ok(*value),
        CpuExtent::Symbol { name } => dimensions
            .get(name)
            .copied()
            .map(|value| u64::try_from(value).unwrap_or(u64::MAX))
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::CpuExecutionInputMismatch,
                    "CPU measurement symbolic extent is unbound",
                )
            }),
    }
}

fn projected_instruction_work(
    package: &CpuArtifactPackage,
    inputs: &BTreeMap<String, Value>,
) -> AgentResult<u64> {
    let (dimensions, _) = tensor_dimensions(package, inputs)?;
    let function = package.functions.first().ok_or_else(|| {
        AgentError::new(
            ErrorCode::CpuArtifactInvalid,
            "CPU package has no entry function",
        )
    })?;
    let mut work = u64::try_from(function.instructions.len()).map_err(|_| {
        AgentError::new(
            ErrorCode::CpuMeasurementOverflow,
            "CPU instruction count overflow",
        )
    })?;
    for instruction in &function.instructions {
        if let CpuInstruction::MapF32 { extent, body, .. }
        | CpuInstruction::ZipMapF32 { extent, body, .. } = instruction
        {
            let body_len = u64::try_from(body.instructions.len()).map_err(|_| {
                AgentError::new(
                    ErrorCode::CpuMeasurementOverflow,
                    "CPU scalar body count overflow",
                )
            })?;
            work = work
                .checked_add(
                    resolved_extent(extent, &dimensions)?
                        .checked_mul(body_len)
                        .ok_or_else(|| {
                            AgentError::new(
                                ErrorCode::CpuMeasurementOverflow,
                                "CPU projected instruction work overflow",
                            )
                        })?,
                )
                .ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::CpuMeasurementOverflow,
                        "CPU projected instruction work overflow",
                    )
                })?;
        }
    }
    Ok(work)
}

/// Acquires one production-clock CPU measurement. This is the sole protocol clock boundary.
pub fn acquire(
    package: &CpuArtifactPackage,
    config: CpuBenchmarkConfig,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
) -> AgentResult<CpuMeasurementDraft> {
    let mut clock = MonotonicClock::default();
    acquire_with_clock(package, config, inputs, limits, &mut clock)
}

/// Acquires a measurement with an injectable clock for explicit tests and fixtures.
pub fn acquire_with_clock<C: CpuClock>(
    package: &CpuArtifactPackage,
    config: CpuBenchmarkConfig,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
    clock: &mut C,
) -> AgentResult<CpuMeasurementDraft> {
    acquire_with_components(
        package,
        config,
        inputs,
        limits,
        clock,
        &mut agentir_backend_cpu::execute,
    )
}

/// Acquires a deterministic fixture measurement with explicit clock and execution doubles.
///
/// This seam is intentionally absent from the production protocol. It exists so closure
/// tests can prove exact interpreter and clock call counts without adding persisted state.
#[doc(hidden)]
pub fn acquire_with_test_doubles<C: CpuClock, E: CpuExecutionTestDouble>(
    package: &CpuArtifactPackage,
    config: CpuBenchmarkConfig,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
    clock: &mut C,
    executor: &mut E,
) -> AgentResult<CpuMeasurementDraft> {
    acquire_with_components(
        package,
        config,
        inputs,
        limits,
        clock,
        &mut |package, inputs, limits| executor.execute(package, inputs, limits),
    )
}

fn acquire_with_components<C, F>(
    package: &CpuArtifactPackage,
    config: CpuBenchmarkConfig,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
    clock: &mut C,
    execute: &mut F,
) -> AgentResult<CpuMeasurementDraft>
where
    C: CpuClock,
    F: FnMut(
        &CpuArtifactPackage,
        &BTreeMap<String, Value>,
        &ResourceLimits,
    ) -> AgentResult<agentir_backend_cpu::CpuExecutionResult>,
{
    let executions = validate_cpu_benchmark_config(&config, limits)?;
    let (_, input_elements) = tensor_dimensions(package, inputs)?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionElements,
        input_elements,
        "CPU measurement input elements",
    )?;
    let input_bytes = input_elements.checked_mul(4).ok_or_else(|| {
        AgentError::new(
            ErrorCode::CpuMeasurementOverflow,
            "CPU measurement input byte projection overflow",
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionBytes,
        input_bytes,
        "CPU measurement input bytes",
    )?;
    let per_execution = projected_instruction_work(package, inputs)?;
    let projected = per_execution.checked_mul(executions).ok_or_else(|| {
        AgentError::new(
            ErrorCode::CpuMeasurementOverflow,
            "CPU measurement projected work overflow",
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionElements,
        projected,
        "CPU measurement projected instruction work",
    )?;

    let mut agreed_outputs = None;
    for _ in 0..config.warmups {
        let result = execute(package, inputs, limits)?;
        if agreed_outputs
            .as_ref()
            .is_some_and(|outputs| outputs != &result.outputs)
        {
            return Err(AgentError::new(
                ErrorCode::CpuMeasurementOutputMismatch,
                "CPU warmup output differs from an earlier execution",
            ));
        }
        agreed_outputs.get_or_insert(result.outputs);
    }
    let mut raw_duration_ns = Vec::with_capacity(usize::try_from(config.iterations).unwrap_or(0));
    for _ in 0..config.iterations {
        let before = clock.now_ns()?;
        let result = execute(package, inputs, limits)?;
        let after = clock.now_ns()?;
        let elapsed = after.checked_sub(before).ok_or_else(|| {
            AgentError::new(
                ErrorCode::CpuMeasurementOverflow,
                "CPU measurement clock regressed",
            )
        })?;
        if agreed_outputs
            .as_ref()
            .is_some_and(|outputs| outputs != &result.outputs)
        {
            return Err(AgentError::new(
                ErrorCode::CpuMeasurementOutputMismatch,
                "CPU measured iteration output differs from an earlier execution",
            ));
        }
        agreed_outputs.get_or_insert(result.outputs);
        raw_duration_ns.push(elapsed);
    }
    let outputs = agreed_outputs.ok_or_else(|| {
        AgentError::new(
            ErrorCode::CpuMeasurementConfigInvalid,
            "CPU measurement produced no execution output",
        )
    })?;
    let aggregates = aggregate_cpu_durations(&raw_duration_ns)?;
    Ok(CpuMeasurementDraft {
        cpu_artifact: package.id.clone(),
        cpu_artifact_hash: package.cpu_artifact_hash.clone(),
        compiler_build_hash: package.compiler_build_hash.clone(),
        runtime_version: CPU_MEASUREMENT_RUNTIME_VERSION.to_owned(),
        config,
        inputs: inputs.clone(),
        host: host_descriptor(clock.source()),
        raw_duration_ns,
        aggregates,
        outputs,
    })
}
