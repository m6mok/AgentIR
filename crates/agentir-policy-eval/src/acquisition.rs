//! Stage 7C reproducible hardware-measurement acquisition contracts.
//!
//! Acquisition is an explicit evaluation-only orchestration layer.  The
//! compiler remains responsible for artifacts and completed measurement
//! records; this module owns only bounded plans, slot order, resumability and
//! non-correctness provenance.

use crate::{
    hashing::domain_hash_cleared,
    measured::MeasurementValidationPolicy,
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, EvaluationTaskId},
};
use agentir_core::{
    Workspace,
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig},
    ids::ArtifactId,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Measurement-acquisition plan hash domain.
pub const MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_plan.v1\0";

/// V1 deterministic acquisition ordering policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionOrderingPolicy {
    /// Visit every artifact in hash order in each successive round.
    RoundRobinArtifactHashV1,
}

/// V1 behavior after a benchmark slot fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionFailurePolicy {
    /// Stop after retaining the typed failed slot outcome.
    StopOnFirstFailureV1,
}

/// Compiler-owned artifact facts used to construct an acquisition plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionArtifact {
    /// Compiler-assigned artifact identity.
    pub artifact_id: ArtifactId,
    /// Exact compiler-owned artifact hash.
    pub artifact_hash: String,
    /// Frozen semantic identity shared by the requested artifacts.
    pub spec_hash: String,
    /// Immutable target identity shared by the requested artifacts.
    pub target_hash: String,
    /// Compiler build that emitted the artifact.
    pub compiler_build_hash: String,
    /// Retained compiler-owned lifecycle status.
    pub status: ArtifactStatus,
    /// Whether production structural verification succeeded.
    pub offline_valid: bool,
}

/// Server-owned view of one exact production workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionCatalog {
    /// Exact workspace identity.
    pub workspace_id: String,
    /// Exact immutable root anchor selected by the evaluation task.
    pub root_anchor_hash: String,
    /// Artifacts keyed by compiler-owned hash.
    pub artifacts: BTreeMap<String, MeasurementAcquisitionArtifact>,
    /// Explicit fixture marker; production catalogs always set this to false.
    pub synthetic_fixture: bool,
}

impl MeasurementAcquisitionCatalog {
    /// Builds a catalog by fully checking every retained production artifact.
    pub fn from_workspace(
        workspace: &Workspace,
        root_anchor_hash: String,
    ) -> EvaluationResult<Self> {
        let mut artifacts = BTreeMap::new();
        for package in workspace.artifact_store().packages.values() {
            let report = workspace.artifact_check(&package.id).map_err(|error| {
                acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactInvalid,
                    format!("production artifact verification failed: {error}"),
                )
            })?;
            let artifact = MeasurementAcquisitionArtifact {
                artifact_id: package.id.clone(),
                artifact_hash: package.artifact_hash.to_string(),
                spec_hash: package.manifest.anchor.spec_hash.to_string(),
                target_hash: package.manifest.anchor.target_hash.to_string(),
                compiler_build_hash: package.manifest.compiler_build_hash.to_string(),
                status: package.status,
                offline_valid: report.offline_valid && report.equivalent_to_backend,
            };
            if artifacts
                .insert(artifact.artifact_hash.clone(), artifact)
                .is_some()
            {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                    "production workspace contains duplicate artifact hashes",
                ));
            }
        }
        Ok(Self {
            workspace_id: workspace.id().to_string(),
            root_anchor_hash,
            artifacts,
            synthetic_fixture: false,
        })
    }

    /// Constructs an explicitly labelled test/study-only fixture catalog.
    ///
    /// Fixture catalogs cannot be used with the hardware validation policy.
    pub fn synthetic_fixture(
        workspace_id: String,
        root_anchor_hash: String,
        artifacts: Vec<MeasurementAcquisitionArtifact>,
    ) -> EvaluationResult<Self> {
        let mut by_hash = BTreeMap::new();
        for artifact in artifacts {
            if by_hash
                .insert(artifact.artifact_hash.clone(), artifact)
                .is_some()
            {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                    "synthetic catalog contains duplicate artifact hashes",
                ));
            }
        }
        Ok(Self {
            workspace_id,
            root_anchor_hash,
            artifacts: by_hash,
            synthetic_fixture: true,
        })
    }
}

/// Client-visible control input for one immutable acquisition plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementAcquisitionPlanRequest {
    /// Exact evaluation corpus anchor.
    pub corpus_hash: String,
    /// Exact task identity.
    pub task_id: EvaluationTaskId,
    /// Exact production root anchor.
    pub root_anchor_hash: String,
    /// Requested compiler-owned artifact hashes; request order is ignored.
    pub artifact_hashes: Vec<String>,
    /// Server-validated benchmark configuration request.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Fixed positive number of records per artifact.
    pub records_per_artifact: u64,
    /// Validation policy for resulting records.
    pub validation_policy: MeasurementValidationPolicy,
    /// V1 acquisition ordering policy.
    pub ordering_policy: MeasurementAcquisitionOrderingPolicy,
    /// V1 failure policy.
    pub failure_policy: MeasurementAcquisitionFailurePolicy,
    /// Positive automatic checkpoint cadence in completed slots.
    pub checkpoint_cadence_slots: u64,
}

/// Immutable Stage 7C measurement-acquisition plan v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPlan {
    /// Plan schema version.
    pub version: u32,
    /// Exact production workspace identity.
    pub workspace_id: String,
    /// Exact immutable root anchor.
    pub root_anchor_hash: String,
    /// Exact evaluation corpus anchor.
    pub corpus_hash: String,
    /// Exact task identity.
    pub task_id: EvaluationTaskId,
    /// Canonical compiler-owned artifact-hash order.
    pub artifact_hashes: Vec<String>,
    /// Semantic identity shared by every artifact.
    pub spec_hash: String,
    /// Target identity shared by every artifact.
    pub target_hash: String,
    /// Compiler build shared by every artifact.
    pub compiler_build_hash: String,
    /// Exact requested benchmark configuration.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Fixed positive number of records per artifact.
    pub records_per_artifact: u64,
    /// Validation policy for completed production records.
    pub validation_policy: MeasurementValidationPolicy,
    /// Exact deterministic ordering contract.
    pub ordering_policy: MeasurementAcquisitionOrderingPolicy,
    /// Exact failure behavior.
    pub failure_policy: MeasurementAcquisitionFailurePolicy,
    /// Automatic checkpoint cadence measured in completed slots.
    pub checkpoint_cadence_slots: u64,
    /// Independent plan hash.
    pub measurement_acquisition_plan_hash: String,
}

impl MeasurementAcquisitionPlan {
    /// Resolves, validates and canonicalizes one plan against a server-owned catalog.
    pub fn new(
        catalog: &MeasurementAcquisitionCatalog,
        request: MeasurementAcquisitionPlanRequest,
    ) -> EvaluationResult<Self> {
        if request.root_anchor_hash != catalog.root_anchor_hash {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "acquisition root anchor is stale for the production workspace",
            ));
        }
        if request.artifact_hashes.is_empty()
            || request.records_per_artifact == 0
            || request.checkpoint_cadence_slots == 0
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionPlanInvalid,
                "acquisition requires artifacts and positive record/checkpoint counts",
            ));
        }
        validate_benchmark_config(&request.benchmark_config)?;
        if catalog.synthetic_fixture
            && request.validation_policy == MeasurementValidationPolicy::HardwareExecutedV1
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode,
                "synthetic fixture catalogs cannot claim hardware validation",
            ));
        }
        let mut artifact_hashes = request.artifact_hashes;
        artifact_hashes.sort();
        if artifact_hashes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "duplicate artifact hashes are forbidden",
            ));
        }
        let mut resolved = Vec::with_capacity(artifact_hashes.len());
        for hash in &artifact_hashes {
            let artifact = catalog.artifacts.get(hash).ok_or_else(|| {
                acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                    format!("artifact hash `{hash}` is absent from the production workspace"),
                )
            })?;
            if !artifact.offline_valid
                || !matches!(
                    artifact.status,
                    ArtifactStatus::Validated | ArtifactStatus::Sealed
                )
            {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactInvalid,
                    "acquisition requires proved and offline-valid terminal artifacts",
                ));
            }
            resolved.push(artifact);
        }
        let spec_hashes = resolved
            .iter()
            .map(|artifact| artifact.spec_hash.as_str())
            .collect::<BTreeSet<_>>();
        if spec_hashes.len() != 1 {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionMixedSpec,
                "acquisition artifacts do not share one spec hash",
            ));
        }
        let target_hashes = resolved
            .iter()
            .map(|artifact| artifact.target_hash.as_str())
            .collect::<BTreeSet<_>>();
        if target_hashes.len() != 1 {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionMixedTarget,
                "acquisition artifacts do not share one target hash",
            ));
        }
        let build_hashes = resolved
            .iter()
            .map(|artifact| artifact.compiler_build_hash.as_str())
            .collect::<BTreeSet<_>>();
        if build_hashes.len() != 1 {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCompilerBuildChanged,
                "acquisition artifacts do not share one compiler build",
            ));
        }
        let mut plan = Self {
            version: 1,
            workspace_id: catalog.workspace_id.clone(),
            root_anchor_hash: catalog.root_anchor_hash.clone(),
            corpus_hash: request.corpus_hash,
            task_id: request.task_id,
            artifact_hashes,
            spec_hash: resolved[0].spec_hash.clone(),
            target_hash: resolved[0].target_hash.clone(),
            compiler_build_hash: resolved[0].compiler_build_hash.clone(),
            benchmark_config: request.benchmark_config,
            records_per_artifact: request.records_per_artifact,
            validation_policy: request.validation_policy,
            ordering_policy: request.ordering_policy,
            failure_policy: request.failure_policy,
            checkpoint_cadence_slots: request.checkpoint_cadence_slots,
            measurement_acquisition_plan_hash: String::new(),
        };
        plan.measurement_acquisition_plan_hash = measurement_acquisition_plan_hash(&plan)?;
        Ok(plan)
    }

    /// Returns the exact number of canonical slots using checked arithmetic.
    pub fn total_slots(&self) -> EvaluationResult<u64> {
        u64::try_from(self.artifact_hashes.len())
            .unwrap_or(u64::MAX)
            .checked_mul(self.records_per_artifact)
            .ok_or_else(acquisition_overflow)
    }

    /// Returns the exact canonical round-robin slot descriptors.
    pub fn slots(&self) -> EvaluationResult<Vec<MeasurementAcquisitionPlannedSlot>> {
        let capacity = usize::try_from(self.total_slots()?).map_err(|_| acquisition_overflow())?;
        let mut slots = Vec::with_capacity(capacity);
        for round_index in 0..self.records_per_artifact {
            for artifact_hash in &self.artifact_hashes {
                let slot_index = u64::try_from(slots.len()).map_err(|_| acquisition_overflow())?;
                slots.push(MeasurementAcquisitionPlannedSlot {
                    slot_index,
                    round_index,
                    artifact_hash: artifact_hash.clone(),
                });
            }
        }
        Ok(slots)
    }

    /// Revalidates the plan hash and canonical fields without hardware work.
    pub fn verify(&self) -> EvaluationResult<()> {
        if self.version != 1
            || self.artifact_hashes.is_empty()
            || self.records_per_artifact == 0
            || self.checkpoint_cadence_slots == 0
            || self
                .artifact_hashes
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.measurement_acquisition_plan_hash != measurement_acquisition_plan_hash(self)?
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionPlanCorrupt,
                "measurement acquisition plan is corrupt or non-canonical",
            ));
        }
        validate_benchmark_config(&self.benchmark_config)?;
        self.total_slots()?;
        Ok(())
    }
}

/// One canonical slot in the immutable plan schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPlannedSlot {
    /// Zero-based canonical slot index.
    pub slot_index: u64,
    /// Zero-based record round.
    pub round_index: u64,
    /// Compiler-owned artifact hash measured in this slot.
    pub artifact_hash: String,
}

/// Computes the independent acquisition-plan hash.
pub fn measurement_acquisition_plan_hash(
    plan: &MeasurementAcquisitionPlan,
) -> EvaluationResult<String> {
    domain_hash_cleared(MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN, plan, |model| {
        model.measurement_acquisition_plan_hash.clear();
    })
}

fn validate_benchmark_config(config: &HardwareBenchmarkConfig) -> EvaluationResult<()> {
    if config.iterations == 0
        || config.input_distribution.is_empty()
        || config.tensor_dimensions.is_empty()
        || config.tensor_dimensions.contains(&0)
    {
        return Err(acquisition_error(
            EvaluationErrorCode::EvaluationAcquisitionBenchmarkConfigInvalid,
            "benchmark config requires positive iterations, dimensions, and a distribution",
        )
        .expected_actual(
            json!({"iterations":">0","tensor_dimensions":"non_empty_positive"}),
            json!(config),
        ));
    }
    Ok(())
}

fn acquisition_overflow() -> EvaluationDiagnostic {
    acquisition_error(
        EvaluationErrorCode::EvaluationAcquisitionCounterOverflow,
        "checked acquisition counter overflow",
    )
}

fn acquisition_error(
    code: EvaluationErrorCode,
    message: impl Into<String>,
) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}
