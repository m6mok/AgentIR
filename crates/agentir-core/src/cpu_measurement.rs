//! Compiler-owned Stage 8B CPU measurement records and deterministic validation.

use crate::{
    cpu::{CpuArtifactHash, CpuArtifactPackage, CpuArtifactStore, CpuCompilerBuildHash},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{CpuArtifactId, CpuMeasurementId},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, fmt::Write as _};

/// CPU benchmark configuration codec version.
pub const CPU_BENCHMARK_CONFIG_VERSION: u32 = 1;
/// CPU measurement record codec version.
pub const CPU_MEASUREMENT_FORMAT_VERSION: u32 = 1;
/// CPU measurement event replay semantics version.
pub const CPU_MEASUREMENT_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for benchmark configuration identity.
pub const CPU_BENCHMARK_CONFIG_HASH_DOMAIN: &[u8] = b"agentir.cpu.benchmark.config.v1\0";
/// Domain separator for canonical runtime input identity.
pub const CPU_INPUT_HASH_DOMAIN: &[u8] = b"agentir.cpu.benchmark.input.v1\0";
/// Domain separator for runtime-owned host identity.
pub const CPU_HOST_FINGERPRINT_HASH_DOMAIN: &[u8] = b"agentir.cpu.host.fingerprint.v1\0";
/// Domain separator for complete CPU measurement identity.
pub const CPU_MEASUREMENT_HASH_DOMAIN: &[u8] = b"agentir.cpu.measurement.v1\0";
/// Domain separator for deterministic output anchoring.
pub const CPU_OUTPUT_HASH_DOMAIN: &[u8] = b"agentir.cpu.measurement.output.v1\0";

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
    CpuBenchmarkConfigHash,
    "Identity of one canonical bounded CPU benchmark configuration."
);
hash_type!(
    CpuInputHash,
    "Identity of canonical ordinary runtime inputs."
);
hash_type!(
    CpuHostFingerprintHash,
    "Identity of a runtime-owned CPU host descriptor."
);
hash_type!(
    CpuMeasurementHash,
    "Identity of one complete CPU timing observation."
);
hash_type!(CpuOutputHash, "Deterministic anchor for iteration outputs.");

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

fn encoded<T: Serialize>(value: &T, context: &str) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| {
        AgentError::new(
            ErrorCode::CpuMeasurementHashMismatch,
            format!("{context} encoding failed: {error}"),
        )
    })
}

/// Sole Stage 8B integer aggregation method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuAggregationMethod {
    /// Sort integer nanoseconds and compute exact min/median/nearest-rank-p95/max.
    OrderedIntegerNsV1,
}

/// Bounded client-selectable benchmark configuration canonicalized by the compiler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuBenchmarkConfig {
    /// Configuration codec version, always one.
    pub format_version: u32,
    /// Untimed executions before measurement.
    pub warmups: u32,
    /// Timed executions retained as ordered raw samples.
    pub iterations: u32,
    /// Deterministic aggregation method.
    pub aggregation: CpuAggregationMethod,
}

impl CpuBenchmarkConfig {
    /// Creates canonical v1 configuration from client-selectable bounded values.
    #[must_use]
    pub const fn v1(warmups: u32, iterations: u32) -> Self {
        Self {
            format_version: CPU_BENCHMARK_CONFIG_VERSION,
            warmups,
            iterations,
            aggregation: CpuAggregationMethod::OrderedIntegerNsV1,
        }
    }
}

/// Identifies whether the clock is the production monotonic source or an explicit test fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuClockSource {
    /// Process-local monotonic production clock.
    ProductionMonotonicV1,
    /// Deterministic synthetic clock accepted only by explicit test APIs/fixtures.
    SyntheticTestFixtureV1,
}

/// Runtime-owned host and environment descriptor without paths, process IDs, or timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuHostDescriptor {
    /// Descriptor codec version.
    pub format_version: u32,
    /// Rust target operating system.
    pub operating_system: String,
    /// Rust target architecture.
    pub architecture: String,
    /// Rust target family.
    pub family: String,
    /// Stage 8B runtime contract version.
    pub runtime_version: String,
    /// Explicit clock provenance.
    pub clock_source: CpuClockSource,
}

/// Deterministic aggregate statistics over ordered raw integer-nanosecond samples.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuDurationAggregates {
    /// Smallest sample.
    pub min_ns: u64,
    /// Integer median (floor of the midpoint for an even sample count).
    pub median_ns: u64,
    /// Nearest-rank 95th percentile.
    pub p95_ns: u64,
    /// Largest sample.
    pub max_ns: u64,
}

/// Runtime-created data accepted by the compiler-owned atomic publisher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuMeasurementDraft {
    /// Measured compiler-published artifact.
    pub cpu_artifact: CpuArtifactId,
    /// Exact artifact identity observed before execution.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Exact compiler build identity retained by the package.
    pub compiler_build_hash: CpuCompilerBuildHash,
    /// Runtime contract version.
    pub runtime_version: String,
    /// Canonical bounded configuration.
    pub config: CpuBenchmarkConfig,
    /// Ordinary runtime inputs retained for independent hashing/replay verification.
    pub inputs: BTreeMap<String, Value>,
    /// Runtime-owned host descriptor.
    pub host: CpuHostDescriptor,
    /// Ordered measured durations in integer nanoseconds.
    pub raw_duration_ns: Vec<u64>,
    /// Deterministic aggregates over the raw samples.
    pub aggregates: CpuDurationAggregates,
    /// Deterministic outputs agreed by all iterations.
    pub outputs: BTreeMap<String, Value>,
}

/// Complete immutable Stage 8B non-correctness observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuMeasurementRecord {
    /// Compiler-assigned store identity, excluded from measurement identity.
    pub id: CpuMeasurementId,
    /// Stable record discriminator.
    pub format: String,
    /// Record codec version.
    pub format_version: u32,
    /// Measured compiler-published artifact.
    pub cpu_artifact: CpuArtifactId,
    /// Exact artifact identity.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Exact CPU compiler build identity.
    pub compiler_build_hash: CpuCompilerBuildHash,
    /// Runtime contract version.
    pub runtime_version: String,
    /// Canonical benchmark configuration.
    pub config: CpuBenchmarkConfig,
    /// Independent configuration identity.
    pub cpu_benchmark_config_hash: CpuBenchmarkConfigHash,
    /// Canonical ordinary runtime inputs.
    pub inputs: BTreeMap<String, Value>,
    /// Independent input identity.
    pub cpu_input_hash: CpuInputHash,
    /// Runtime-owned host descriptor.
    pub host: CpuHostDescriptor,
    /// Independent host descriptor identity.
    pub cpu_host_fingerprint_hash: CpuHostFingerprintHash,
    /// Ordered raw integer-nanosecond samples.
    pub raw_duration_ns: Vec<u64>,
    /// Deterministic aggregates.
    pub aggregates: CpuDurationAggregates,
    /// Deterministic outputs agreed by every execution.
    pub outputs: BTreeMap<String, Value>,
    /// Independent deterministic output anchor.
    pub output_hash: CpuOutputHash,
    /// Complete measurement identity, excluding `id` and resource policy.
    pub cpu_measurement_hash: CpuMeasurementHash,
}

#[derive(Serialize)]
struct MeasurementHashModel<'a> {
    format: &'a str,
    format_version: u32,
    cpu_artifact: &'a CpuArtifactId,
    cpu_artifact_hash: &'a CpuArtifactHash,
    compiler_build_hash: &'a CpuCompilerBuildHash,
    runtime_version: &'a str,
    config: &'a CpuBenchmarkConfig,
    cpu_benchmark_config_hash: &'a CpuBenchmarkConfigHash,
    inputs: &'a BTreeMap<String, Value>,
    cpu_input_hash: &'a CpuInputHash,
    host: &'a CpuHostDescriptor,
    cpu_host_fingerprint_hash: &'a CpuHostFingerprintHash,
    raw_duration_ns: &'a [u64],
    aggregates: &'a CpuDurationAggregates,
    outputs: &'a BTreeMap<String, Value>,
    output_hash: &'a CpuOutputHash,
}

/// Computes the independent configuration hash.
pub fn cpu_benchmark_config_hash(
    config: &CpuBenchmarkConfig,
) -> AgentResult<CpuBenchmarkConfigHash> {
    Ok(CpuBenchmarkConfigHash(digest(
        CPU_BENCHMARK_CONFIG_HASH_DOMAIN,
        &encoded(config, "CPU benchmark configuration")?,
    )))
}

/// Computes the independent canonical input hash.
pub fn cpu_input_hash(inputs: &BTreeMap<String, Value>) -> AgentResult<CpuInputHash> {
    Ok(CpuInputHash(digest(
        CPU_INPUT_HASH_DOMAIN,
        &encoded(inputs, "CPU benchmark inputs")?,
    )))
}

/// Computes the independent runtime-owned host fingerprint.
pub fn cpu_host_fingerprint_hash(host: &CpuHostDescriptor) -> AgentResult<CpuHostFingerprintHash> {
    Ok(CpuHostFingerprintHash(digest(
        CPU_HOST_FINGERPRINT_HASH_DOMAIN,
        &encoded(host, "CPU host descriptor")?,
    )))
}

/// Computes a deterministic output anchor.
pub fn cpu_output_hash(outputs: &BTreeMap<String, Value>) -> AgentResult<CpuOutputHash> {
    Ok(CpuOutputHash(digest(
        CPU_OUTPUT_HASH_DOMAIN,
        &encoded(outputs, "CPU measurement outputs")?,
    )))
}

/// Computes the complete measurement hash without its store-local ID.
pub fn cpu_measurement_hash(record: &CpuMeasurementRecord) -> AgentResult<CpuMeasurementHash> {
    let model = MeasurementHashModel {
        format: &record.format,
        format_version: record.format_version,
        cpu_artifact: &record.cpu_artifact,
        cpu_artifact_hash: &record.cpu_artifact_hash,
        compiler_build_hash: &record.compiler_build_hash,
        runtime_version: &record.runtime_version,
        config: &record.config,
        cpu_benchmark_config_hash: &record.cpu_benchmark_config_hash,
        inputs: &record.inputs,
        cpu_input_hash: &record.cpu_input_hash,
        host: &record.host,
        cpu_host_fingerprint_hash: &record.cpu_host_fingerprint_hash,
        raw_duration_ns: &record.raw_duration_ns,
        aggregates: &record.aggregates,
        outputs: &record.outputs,
        output_hash: &record.output_hash,
    };
    Ok(CpuMeasurementHash(digest(
        CPU_MEASUREMENT_HASH_DOMAIN,
        &encoded(&model, "CPU measurement")?,
    )))
}

/// Validates bounded configuration without adding resource policy to identity.
pub fn validate_cpu_benchmark_config(
    config: &CpuBenchmarkConfig,
    limits: &ResourceLimits,
) -> AgentResult<u64> {
    if config.format_version != CPU_BENCHMARK_CONFIG_VERSION || config.iterations == 0 {
        return Err(AgentError::new(
            ErrorCode::CpuMeasurementConfigInvalid,
            "CPU measurement requires config v1 and at least one measured iteration",
        ));
    }
    BudgetCheck::against(
        limits,
        ResourceKind::BenchmarkWarmups,
        u64::from(config.warmups),
        "CPU measurement warmups",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::BenchmarkIterations,
        u64::from(config.iterations),
        "CPU measurement iterations",
    )?;
    u64::from(config.warmups)
        .checked_add(u64::from(config.iterations))
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::CpuMeasurementOverflow,
                "CPU measurement execution count overflow",
            )
        })
}

/// Computes deterministic integer-nanosecond aggregates.
pub fn aggregate_cpu_durations(samples: &[u64]) -> AgentResult<CpuDurationAggregates> {
    if samples.is_empty() {
        return Err(AgentError::new(
            ErrorCode::CpuMeasurementConfigInvalid,
            "CPU measurement samples cannot be empty",
        ));
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    let median_ns = if sorted.len() % 2 == 0 {
        let low = sorted[middle - 1];
        low + (sorted[middle] - low) / 2
    } else {
        sorted[middle]
    };
    let count = u64::try_from(sorted.len()).map_err(|_| {
        AgentError::new(
            ErrorCode::CpuMeasurementOverflow,
            "CPU measurement sample count overflow",
        )
    })?;
    let rank = count
        .checked_mul(95)
        .and_then(|value| value.checked_add(99))
        .ok_or_else(|| {
            AgentError::new(ErrorCode::CpuMeasurementOverflow, "CPU p95 rank overflow")
        })?
        / 100;
    let p95_index = usize::try_from(rank.saturating_sub(1)).map_err(|_| {
        AgentError::new(ErrorCode::CpuMeasurementOverflow, "CPU p95 index overflow")
    })?;
    Ok(CpuDurationAggregates {
        min_ns: sorted[0],
        median_ns,
        p95_ns: sorted[p95_index],
        max_ns: sorted[sorted.len() - 1],
    })
}

/// Recomputes every structural and independent hash contract without execution or clock access.
pub fn verify_cpu_measurement(
    record: &CpuMeasurementRecord,
    package: &CpuArtifactPackage,
) -> AgentResult<()> {
    validate_cpu_benchmark_config(&record.config, &ResourceLimits::hard_safety_caps())?;
    if record.format != "agentir.cpu.measurement"
        || record.format_version != CPU_MEASUREMENT_FORMAT_VERSION
        || record.cpu_artifact != package.id
        || record.cpu_artifact_hash != package.cpu_artifact_hash
        || record.compiler_build_hash != package.compiler_build_hash
        || record.runtime_version != record.host.runtime_version
        || record.runtime_version.is_empty()
        || record.host.format_version != 1
        || record.host.operating_system.is_empty()
        || record.host.architecture.is_empty()
        || record.host.family.is_empty()
        || record.config.format_version != CPU_BENCHMARK_CONFIG_VERSION
        || record.raw_duration_ns.len()
            != usize::try_from(record.config.iterations).unwrap_or(usize::MAX)
    {
        return Err(AgentError::new(
            ErrorCode::CpuMeasurementHashMismatch,
            "CPU measurement format, artifact provenance, runtime, or sample count is invalid",
        ));
    }
    if record.cpu_benchmark_config_hash != cpu_benchmark_config_hash(&record.config)?
        || record.cpu_input_hash != cpu_input_hash(&record.inputs)?
        || record.cpu_host_fingerprint_hash != cpu_host_fingerprint_hash(&record.host)?
        || record.aggregates != aggregate_cpu_durations(&record.raw_duration_ns)?
        || record.output_hash != cpu_output_hash(&record.outputs)?
        || record.cpu_measurement_hash != cpu_measurement_hash(record)?
    {
        return Err(AgentError::new(
            ErrorCode::CpuMeasurementHashMismatch,
            "CPU measurement hash-covered data is inconsistent",
        ));
    }
    Ok(())
}

/// Replayable atomic measurement publication event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuMeasurementEvent {
    /// Exact published record.
    pub record: CpuMeasurementRecord,
    /// CPU artifact event dependency cursor.
    pub cpu_artifact_event_cursor: u64,
}

/// Measurement event paired with independent replay semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionedCpuMeasurementEvent {
    /// Event semantics version.
    pub semantics_version: u32,
    /// Replayable event.
    pub event: CpuMeasurementEvent,
}

/// Separate persistent Stage 8B measurement store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuMeasurementStore {
    /// Compiler-assigned next measurement sequence.
    pub next_id: u64,
    /// Immutable records by compiler-owned ID.
    pub records: BTreeMap<CpuMeasurementId, CpuMeasurementRecord>,
    /// Ordered publication events.
    pub events: Vec<VersionedCpuMeasurementEvent>,
}

impl CpuMeasurementStore {
    /// Atomically validates and publishes one runtime-created draft.
    pub fn publish(
        &mut self,
        artifacts: &CpuArtifactStore,
        draft: CpuMeasurementDraft,
        artifact_event_cursor: u64,
    ) -> AgentResult<CpuMeasurementRecord> {
        let package = artifacts.package(&draft.cpu_artifact)?;
        if package.cpu_artifact_hash != draft.cpu_artifact_hash
            || package.compiler_build_hash != draft.compiler_build_hash
        {
            return Err(AgentError::new(
                ErrorCode::CpuArtifactHashMismatch,
                "CPU measurement draft does not match the retained artifact",
            ));
        }
        if let Some(source) = self
            .records
            .values()
            .next()
            .map(|record| record.host.clock_source)
        {
            if source != draft.host.clock_source {
                return Err(AgentError::new(
                    ErrorCode::CpuMeasurementConfigInvalid,
                    "production and synthetic fixture measurements cannot share one store",
                ));
            }
        }
        let next_id = self.next_id.checked_add(1).ok_or_else(|| {
            AgentError::new(
                ErrorCode::CpuMeasurementOverflow,
                "CPU measurement ID allocator overflow",
            )
        })?;
        let id = CpuMeasurementId::new(format!("cpum{next_id}"));
        let mut record = CpuMeasurementRecord {
            id: id.clone(),
            format: "agentir.cpu.measurement".to_owned(),
            format_version: CPU_MEASUREMENT_FORMAT_VERSION,
            cpu_artifact: draft.cpu_artifact,
            cpu_artifact_hash: draft.cpu_artifact_hash,
            compiler_build_hash: draft.compiler_build_hash,
            runtime_version: draft.runtime_version,
            cpu_benchmark_config_hash: cpu_benchmark_config_hash(&draft.config)?,
            config: draft.config,
            cpu_input_hash: cpu_input_hash(&draft.inputs)?,
            inputs: draft.inputs,
            cpu_host_fingerprint_hash: cpu_host_fingerprint_hash(&draft.host)?,
            host: draft.host,
            raw_duration_ns: draft.raw_duration_ns,
            aggregates: draft.aggregates,
            output_hash: cpu_output_hash(&draft.outputs)?,
            outputs: draft.outputs,
            cpu_measurement_hash: CpuMeasurementHash::new("pending"),
        };
        record.cpu_measurement_hash = cpu_measurement_hash(&record)?;
        verify_cpu_measurement(&record, package)?;
        self.next_id = next_id;
        self.records.insert(id, record.clone());
        self.events.push(VersionedCpuMeasurementEvent {
            semantics_version: CPU_MEASUREMENT_EVENT_SEMANTICS_VERSION,
            event: CpuMeasurementEvent {
                record: record.clone(),
                cpu_artifact_event_cursor: artifact_event_cursor,
            },
        });
        Ok(record)
    }

    /// Lists immutable records in compiler ID order.
    #[must_use]
    pub fn list(&self) -> Vec<CpuMeasurementRecord> {
        self.records.values().cloned().collect()
    }

    /// Reads one immutable record.
    pub fn query(&self, id: &CpuMeasurementId) -> AgentResult<&CpuMeasurementRecord> {
        self.records.get(id).ok_or_else(|| {
            AgentError::new(
                ErrorCode::CpuMeasurementNotFound,
                format!("CPU measurement `{id}` does not exist"),
            )
        })
    }

    /// Rechecks one record without execution or clock access.
    pub fn check(
        &self,
        id: &CpuMeasurementId,
        artifacts: &CpuArtifactStore,
    ) -> AgentResult<CpuMeasurementRecord> {
        let record = self.query(id)?;
        verify_cpu_measurement(record, artifacts.package(&record.cpu_artifact)?)?;
        Ok(record.clone())
    }
}
