//! Stage 7C reproducible hardware-measurement acquisition contracts.
//!
//! Acquisition is an explicit evaluation-only orchestration layer.  The
//! compiler remains responsible for artifacts and completed measurement
//! records; this module owns only bounded plans, slot order, resumability and
//! non-correctness provenance.

use crate::{
    hashing::domain_hash_cleared,
    measured::{MeasurementCohortRecord, MeasurementValidationPolicy},
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, EvaluationTaskId},
};
use agentir_core::{
    Workspace,
    backend::{MeasurementHash, compiler_build_hash, measurement_hash},
    backend_ir::{
        ArtifactStatus, BackendParameterType, DeviceFingerprint, HardwareBenchmarkConfig,
        HardwareMeasurementRecord, MEASUREMENT_FORMAT_VERSION,
    },
    diagnostics::ErrorCode,
    ids::{ArtifactId, MeasurementId},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

/// Measurement-acquisition plan hash domain.
pub const MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_plan.v1\0";
/// Measurement-acquisition checkpoint hash domain.
pub const MEASUREMENT_ACQUISITION_CHECKPOINT_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_checkpoint.v1\0";
/// Measurement-acquisition trace hash domain.
pub const MEASUREMENT_ACQUISITION_TRACE_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_trace.v1\0";
/// Measurement-acquisition result hash domain.
pub const MEASUREMENT_ACQUISITION_RESULT_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_result.v1\0";

/// Operational Stage 7C safety limits, excluded from every acquisition hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementAcquisitionLimits {
    /// Maximum distinct artifacts in one plan.
    pub artifact_references: u64,
    /// Maximum requested records per artifact.
    pub records_per_artifact: u64,
    /// Maximum canonical slots in one plan.
    pub total_slots: u64,
    /// Maximum encoded checkpoint bytes.
    pub checkpoint_bytes: u64,
}

impl Default for MeasurementAcquisitionLimits {
    fn default() -> Self {
        Self {
            artifact_references: 1_024,
            records_per_artifact: 10_000,
            total_slots: 1_000_000,
            checkpoint_bytes: 256 * 1024 * 1024,
        }
    }
}

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
        Self::new_with_limits(catalog, request, &MeasurementAcquisitionLimits::default())
    }

    /// Resolves one plan under explicit operational limits excluded from its identity.
    pub fn new_with_limits(
        catalog: &MeasurementAcquisitionCatalog,
        request: MeasurementAcquisitionPlanRequest,
        limits: &MeasurementAcquisitionLimits,
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
        let artifact_count =
            u64::try_from(request.artifact_hashes.len()).map_err(|_| acquisition_overflow())?;
        let total_slots = artifact_count
            .checked_mul(request.records_per_artifact)
            .ok_or_else(acquisition_overflow)?;
        if artifact_count > limits.artifact_references
            || request.records_per_artifact > limits.records_per_artifact
            || total_slots > limits.total_slots
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionLimitExceeded,
                "measurement acquisition plan exceeds operational safety limits",
            )
            .expected_actual(
                json!({
                    "artifact_references": limits.artifact_references,
                    "records_per_artifact": limits.records_per_artifact,
                    "total_slots": limits.total_slots,
                }),
                json!({
                    "artifact_references": artifact_count,
                    "records_per_artifact": request.records_per_artifact,
                    "total_slots": total_slots,
                }),
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

/// Lifecycle state of one canonical acquisition slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionSlotStatus {
    /// Slot has not started.
    Pending,
    /// One complete production record was atomically published.
    Complete,
    /// The benchmark completed with a typed failure and no record.
    BenchmarkFailed,
    /// Device became unavailable before publication.
    DeviceUnavailable,
    /// Publication/checkpoint ambiguity requires explicit reconciliation.
    IndeterminateAfterCrash,
}

/// Lifecycle state of one acquisition session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionStatus {
    /// More canonical slots remain.
    Running,
    /// Every planned slot published one complete record.
    Complete,
    /// Cancellation was observed between slots.
    Cancelled,
    /// A compatible device was unavailable.
    DeviceUnavailable,
    /// One slot returned a typed benchmark failure.
    BenchmarkFailed,
    /// A retained anchor changed before hardware work.
    Stale,
    /// A non-benchmark orchestration failure stopped the session.
    Failed,
}

/// Typed reason why acquisition stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionStoppingReason {
    /// All canonical slots completed.
    AllSlotsComplete,
    /// Explicit cooperative cancellation.
    CancelledAtSlotBoundary,
    /// Device was absent or became unavailable.
    DeviceUnavailable,
    /// Hardware benchmark failed without a numerical sentinel.
    BenchmarkFailed,
    /// Immutable acquisition anchors became stale.
    StaleAnchor,
    /// Crash ambiguity requires reconciliation rather than rerun.
    IndeterminateAfterCrash,
}

/// Exact server-owned runtime/device anchors established by preflight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPreflight {
    /// Exact device fingerprint hash.
    pub device_fingerprint_hash: String,
    /// Complete server-owned device provenance.
    pub device: DeviceFingerprint,
    /// Exact compiler build anchor.
    pub compiler_build_hash: String,
    /// Exact runtime implementation anchor.
    pub runtime_version: String,
    /// Stable executor classification.
    pub executor_kind: String,
    /// Whether these observations are explicitly synthetic fixtures.
    pub synthetic_test_data_not_performance_evidence: bool,
}

/// One versioned slot and its immutable expected anchors/outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionSlot {
    /// Slot contract version.
    pub version: u32,
    /// Exact plan anchor.
    pub measurement_acquisition_plan_hash: String,
    /// Canonical zero-based slot index.
    pub slot_index: u64,
    /// Canonical zero-based round index.
    pub round_index: u64,
    /// Exact artifact hash.
    pub artifact_hash: String,
    /// Expected target hash.
    pub target_hash: String,
    /// Expected compiler build hash.
    pub compiler_build_hash: String,
    /// Expected device fingerprint hash.
    pub device_fingerprint_hash: String,
    /// Expected runtime version.
    pub runtime_version: String,
    /// Exact benchmark configuration.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Slot lifecycle status.
    pub status: MeasurementAcquisitionSlotStatus,
    /// Compiler-assigned ID only after successful publication.
    pub measurement_id: Option<MeasurementId>,
    /// Exact measurement hash only after successful publication.
    pub measurement_hash: Option<String>,
    /// Stable typed failure code, never a timing sentinel.
    pub failure_code: Option<EvaluationErrorCode>,
}

/// Deterministic non-correctness acquisition work counters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionWorkCounters {
    /// Complete slots attempted.
    pub slots_attempted: u64,
    /// Complete records published.
    pub slots_completed: u64,
    /// Typed failed slots.
    pub slots_failed: u64,
    /// Indeterminate slots requiring reconciliation.
    pub slots_indeterminate: u64,
    /// Explicit benchmark invocations.
    pub benchmark_invocations: u64,
    /// Concrete runtime/device calls.
    pub device_calls: u64,
    /// Explicit synthetic fixture invocations.
    pub synthetic_fixture_invocations: u64,
    /// Production-format measurement records published.
    pub published_measurement_records: u64,
    /// Completed slots skipped during resume/replay.
    pub prevented_reruns: u64,
    /// Checkpoints created.
    pub checkpoints: u64,
    /// Successful resumes.
    pub resumes: u64,
    /// Cooperative cancellations.
    pub cancellations: u64,
    /// Offline replays.
    pub replays: u64,
    /// Hardware calls during replay; valid replay requires zero.
    pub replay_hardware_calls: u64,
}

/// One deterministic semantic trace event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionTraceEvent {
    /// Zero-based event sequence.
    pub sequence: u64,
    /// Stable event kind.
    pub kind: String,
    /// Canonical slot index when this event concerns a slot.
    pub slot_index: Option<u64>,
    /// Exact artifact hash when this event concerns a slot.
    pub artifact_hash: Option<String>,
    /// Exact measurement hash only for successful publication.
    pub measurement_hash: Option<String>,
    /// Typed failure only for unsuccessful slots.
    pub failure_code: Option<EvaluationErrorCode>,
}

/// Replayable immutable acquisition trace v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionTrace {
    /// Trace contract version.
    pub version: u32,
    /// Exact plan anchor.
    pub measurement_acquisition_plan_hash: String,
    /// Ordered deterministic events.
    pub events: Vec<MeasurementAcquisitionTraceEvent>,
    /// Independent trace hash.
    pub measurement_acquisition_trace_hash: String,
}

/// Stateful in-memory session; mutation occurs only at completed slot boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionSession {
    /// Session contract version.
    pub version: u32,
    /// Deterministic session ID derived from the plan hash.
    pub session_id: String,
    /// Immutable acquisition plan.
    pub plan: MeasurementAcquisitionPlan,
    /// Exact preflight anchors.
    pub preflight: MeasurementAcquisitionPreflight,
    /// Complete canonical slot vector.
    pub slots: Vec<MeasurementAcquisitionSlot>,
    /// Next canonical slot index.
    pub next_slot: u64,
    /// Current lifecycle status.
    pub status: MeasurementAcquisitionStatus,
    /// Terminal stopping reason when stopped.
    pub stopping_reason: Option<MeasurementAcquisitionStoppingReason>,
    /// Cooperative cancellation flag checked between slots.
    pub cancellation_requested: bool,
    /// Deterministic work counters outside semantic identities.
    pub work: MeasurementAcquisitionWorkCounters,
    /// Current replayable semantic trace.
    pub trace: MeasurementAcquisitionTrace,
}

/// Resumable slot-boundary checkpoint v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionCheckpoint {
    /// Checkpoint contract version.
    pub version: u32,
    /// Exact plan anchor.
    pub measurement_acquisition_plan_hash: String,
    /// Exact workspace anchor.
    pub workspace_id: String,
    /// Exact root anchor.
    pub root_anchor_hash: String,
    /// Exact device anchor.
    pub device_fingerprint_hash: String,
    /// Exact compiler build anchor.
    pub compiler_build_hash: String,
    /// Exact runtime anchor.
    pub runtime_version: String,
    /// Next canonical slot.
    pub next_slot: u64,
    /// Completed slot indices in canonical order.
    pub completed_slot_indices: Vec<u64>,
    /// Exact completed measurement IDs.
    pub measurement_ids: Vec<MeasurementId>,
    /// Exact completed measurement hashes.
    pub measurement_hashes: Vec<String>,
    /// Session status at checkpoint time.
    pub status: MeasurementAcquisitionStatus,
    /// Complete in-memory session snapshot.
    pub session: Box<MeasurementAcquisitionSession>,
    /// Independent checkpoint hash.
    pub measurement_acquisition_checkpoint_hash: String,
}

/// Immutable terminal acquisition result v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionResult {
    /// Result contract version.
    pub version: u32,
    /// Exact plan anchor.
    pub measurement_acquisition_plan_hash: String,
    /// Terminal status.
    pub status: MeasurementAcquisitionStatus,
    /// Terminal stopping reason.
    pub stopping_reason: MeasurementAcquisitionStoppingReason,
    /// Completed and failed slots in canonical order.
    pub slots: Vec<MeasurementAcquisitionSlot>,
    /// Ordered successful measurement IDs.
    pub measurement_ids: Vec<MeasurementId>,
    /// Ordered successful measurement hashes.
    pub measurement_hashes: Vec<String>,
    /// Exact completed count per artifact.
    pub completed_records_per_artifact: BTreeMap<String, u64>,
    /// Exact device anchor.
    pub device_fingerprint_hash: String,
    /// Exact compiler build anchor.
    pub compiler_build_hash: String,
    /// Exact runtime anchor.
    pub runtime_version: String,
    /// Exact benchmark configuration.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Deterministic non-correctness work accounting.
    pub work: MeasurementAcquisitionWorkCounters,
    /// Exact trace anchor.
    pub measurement_acquisition_trace_hash: String,
    /// Explicitly non-authoritative provenance.
    pub provenance: String,
    /// Independent result hash.
    pub measurement_acquisition_result_hash: String,
}

/// Atomic bundle attached to evaluation archive v6 after offline replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeasurementAcquisitionArchiveBundle {
    /// Retained stopped sessions.
    pub sessions: Vec<MeasurementAcquisitionSession>,
    /// Exact slot-boundary checkpoints.
    pub checkpoints: Vec<MeasurementAcquisitionCheckpoint>,
    /// Complete production-format records referenced by the sessions.
    pub records: Vec<MeasurementCohortRecord>,
    /// Optional explicit result-hash to Stage 7B cohort-hash links.
    pub cohort_links: BTreeMap<String, String>,
}

/// Server-owned acquisition execution boundary.
pub trait MeasurementAcquisitionExecutor {
    /// Performs device/runtime preflight without publishing a session or record.
    fn preflight(
        &mut self,
        workspace: Option<&Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
    ) -> EvaluationResult<MeasurementAcquisitionPreflight>;

    /// Executes one full benchmark slot and returns one complete unpublished record.
    fn benchmark(
        &mut self,
        workspace: Option<&Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
        preflight: &MeasurementAcquisitionPreflight,
        slot: &MeasurementAcquisitionSlot,
    ) -> EvaluationResult<(HardwareMeasurementRecord, u64)>;
}

/// Explicit deterministic fixture executor for tests and studies only.
#[derive(Clone, Debug)]
pub struct SyntheticMeasurementAcquisitionExecutor {
    /// Server-owned synthetic device fingerprint.
    pub device: DeviceFingerprint,
    /// Stable synthetic runtime anchor.
    pub runtime_version: String,
    /// Optional canonical slot index that returns a benchmark failure.
    pub fail_slot: Option<u64>,
    /// Number of executor invocations, useful for zero-device replay assertions.
    pub invocations: u64,
}

impl SyntheticMeasurementAcquisitionExecutor {
    /// Creates a stable explicitly non-performance fixture executor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            device: DeviceFingerprint {
                backend_api: "synthetic_fixture".to_owned(),
                adapter_name: "synthetic_test_data_not_performance_evidence".to_owned(),
                vendor_id: None,
                device_id: None,
                driver_info: None,
                limits: BTreeMap::new(),
                runtime_version: "synthetic-acquisition-runtime-v1".to_owned(),
                compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            runtime_version: "synthetic-acquisition-runtime-v1".to_owned(),
            fail_slot: None,
            invocations: 0,
        }
    }
}

impl Default for SyntheticMeasurementAcquisitionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl MeasurementAcquisitionExecutor for SyntheticMeasurementAcquisitionExecutor {
    fn preflight(
        &mut self,
        _workspace: Option<&Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
    ) -> EvaluationResult<MeasurementAcquisitionPreflight> {
        if !catalog.synthetic_fixture
            || plan.validation_policy != MeasurementValidationPolicy::SyntheticFixtureV1
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode,
                "synthetic executor is restricted to explicit fixture plans",
            ));
        }
        self.invocations = self
            .invocations
            .checked_add(1)
            .ok_or_else(acquisition_overflow)?;
        Ok(MeasurementAcquisitionPreflight {
            device_fingerprint_hash: agentir_core::backend::device_fingerprint_hash(&self.device)
                .map_err(|error| core_acquisition_error(&error))?
                .to_string(),
            device: self.device.clone(),
            compiler_build_hash: plan.compiler_build_hash.clone(),
            runtime_version: self.runtime_version.clone(),
            executor_kind: "synthetic_fixture_v1".to_owned(),
            synthetic_test_data_not_performance_evidence: true,
        })
    }

    fn benchmark(
        &mut self,
        _workspace: Option<&Workspace>,
        _catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
        preflight: &MeasurementAcquisitionPreflight,
        slot: &MeasurementAcquisitionSlot,
    ) -> EvaluationResult<(HardwareMeasurementRecord, u64)> {
        self.invocations = self
            .invocations
            .checked_add(1)
            .ok_or_else(acquisition_overflow)?;
        if self.fail_slot == Some(slot.slot_index) {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed,
                "synthetic fixture requested a typed benchmark failure",
            ));
        }
        let base = 1_000_u64
            .checked_add(
                slot.slot_index
                    .checked_mul(17)
                    .ok_or_else(acquisition_overflow)?,
            )
            .ok_or_else(acquisition_overflow)?;
        Ok((
            HardwareMeasurementRecord {
                format_version: MEASUREMENT_FORMAT_VERSION,
                artifact_hash: agentir_core::backend::ArtifactHash::new(slot.artifact_hash.clone()),
                target_hash: agentir_core::target::TargetHash::new(plan.target_hash.clone()),
                compiler_build_hash: agentir_core::backend::CompilerBuildHash::new(
                    preflight.compiler_build_hash.clone(),
                ),
                device_fingerprint_hash: agentir_core::backend::DeviceFingerprintHash::new(
                    preflight.device_fingerprint_hash.clone(),
                ),
                device: preflight.device.clone(),
                config: plan.benchmark_config.clone(),
                min_ns: base,
                median_ns: base + 1,
                p95_ns: base + 2,
                max_ns: base + 2,
                guard_outcomes: BTreeMap::from([("synthetic".to_owned(), 1)]),
                validation_status: "synthetic_test_data_not_performance_evidence".to_owned(),
                runtime_version: preflight.runtime_version.clone(),
                measurement_hash: MeasurementHash::new("pending"),
            },
            0,
        ))
    }
}

/// Real-device server-owned WebGPU executor.
#[derive(Clone, Debug)]
pub struct WgpuMeasurementAcquisitionExecutor {
    /// Stable zero-based server-selected adapter.
    pub adapter_index: u32,
}

impl MeasurementAcquisitionExecutor for WgpuMeasurementAcquisitionExecutor {
    fn preflight(
        &mut self,
        workspace: Option<&Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
    ) -> EvaluationResult<MeasurementAcquisitionPreflight> {
        if catalog.synthetic_fixture
            || plan.validation_policy != MeasurementValidationPolicy::HardwareExecutedV1
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode,
                "hardware executor requires a production workspace and hardware policy",
            ));
        }
        let workspace = workspace.ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "hardware executor has no production workspace",
            )
        })?;
        let artifact = catalog
            .artifacts
            .get(&plan.artifact_hashes[0])
            .ok_or_else(|| {
                acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                    "preflight artifact disappeared",
                )
            })?;
        let package = workspace
            .artifact_package(&artifact.artifact_id)
            .map_err(|error| core_acquisition_error(&error))?;
        let target = workspace
            .target_manifest(
                &package.manifest.anchor.target_manifest,
                &package.manifest.anchor.target_revision,
            )
            .map_err(|error| core_acquisition_error(&error))?;
        let device = agentir_runtime_wgpu::list_devices(target)
            .map_err(|error| core_acquisition_error(&error))?
            .into_iter()
            .find(|record| record.index == self.adapter_index && record.target_compatible)
            .ok_or_else(|| {
                acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionDeviceUnavailable,
                    "compatible WebGPU adapter is unavailable",
                )
            })?;
        if compiler_build_hash().to_string() != plan.compiler_build_hash {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCompilerBuildChanged,
                "active compiler build differs from the acquisition plan",
            ));
        }
        Ok(MeasurementAcquisitionPreflight {
            device_fingerprint_hash: device.fingerprint_hash.to_string(),
            device: device.fingerprint,
            compiler_build_hash: plan.compiler_build_hash.clone(),
            runtime_version: agentir_runtime_wgpu::WGPU_RUNTIME_VERSION.to_owned(),
            executor_kind: "hardware_webgpu_v1".to_owned(),
            synthetic_test_data_not_performance_evidence: false,
        })
    }

    fn benchmark(
        &mut self,
        workspace: Option<&Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
        preflight: &MeasurementAcquisitionPreflight,
        slot: &MeasurementAcquisitionSlot,
    ) -> EvaluationResult<(HardwareMeasurementRecord, u64)> {
        let workspace = workspace.ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "hardware benchmark has no production workspace",
            )
        })?;
        let artifact = catalog.artifacts.get(&slot.artifact_hash).ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "benchmark artifact disappeared",
            )
        })?;
        let package = workspace
            .artifact_package(&artifact.artifact_id)
            .map_err(|error| core_acquisition_error(&error))?;
        let target = workspace
            .target_manifest(
                &package.manifest.anchor.target_manifest,
                &package.manifest.anchor.target_revision,
            )
            .map_err(|error| core_acquisition_error(&error))?;
        let inputs = deterministic_runtime_inputs(package, &plan.benchmark_config)?;
        let mut calls = 0_u64;
        for _ in 0..plan.benchmark_config.warmups {
            agentir_runtime_wgpu::execute(package, target, self.adapter_index, &inputs)
                .map_err(|error| core_acquisition_error(&error))?;
            calls = calls.checked_add(1).ok_or_else(acquisition_overflow)?;
        }
        let mut samples = Vec::with_capacity(plan.benchmark_config.iterations as usize);
        let mut last = None;
        let mut guard_outcomes = BTreeMap::new();
        for _ in 0..plan.benchmark_config.iterations {
            let started = Instant::now();
            let execution =
                agentir_runtime_wgpu::execute(package, target, self.adapter_index, &inputs)
                    .map_err(|error| core_acquisition_error(&error))?;
            samples.push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
            let branch = match execution.guard_branch {
                Some(true) => "true",
                Some(false) => "false",
                None => "unguarded",
            };
            *guard_outcomes.entry(branch.to_owned()).or_default() += 1;
            last = Some(execution);
            calls = calls.checked_add(1).ok_or_else(acquisition_overflow)?;
        }
        samples.sort_unstable();
        let last = last.ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionBenchmarkConfigInvalid,
                "benchmark iterations must be positive",
            )
        })?;
        if last.device_fingerprint_hash.to_string() != preflight.device_fingerprint_hash {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionDeviceChanged,
                "device fingerprint changed after preflight",
            ));
        }
        let percentile = |percent: usize| {
            let index = samples.len().saturating_mul(percent).saturating_add(99) / 100;
            samples[index.saturating_sub(1).min(samples.len() - 1)]
        };
        Ok((
            HardwareMeasurementRecord {
                format_version: MEASUREMENT_FORMAT_VERSION,
                artifact_hash: package.artifact_hash.clone(),
                target_hash: package.manifest.anchor.target_hash.clone(),
                compiler_build_hash: package.manifest.compiler_build_hash.clone(),
                device_fingerprint_hash: last.device_fingerprint_hash,
                device: last.device,
                config: plan.benchmark_config.clone(),
                min_ns: samples[0],
                median_ns: percentile(50),
                p95_ns: percentile(95),
                max_ns: samples[samples.len() - 1],
                guard_outcomes,
                validation_status: "offline_validated_and_device_executed".to_owned(),
                runtime_version: preflight.runtime_version.clone(),
                measurement_hash: MeasurementHash::new("pending"),
            },
            calls,
        ))
    }
}

fn deterministic_runtime_inputs(
    package: &agentir_core::backend_ir::ArtifactPackage,
    config: &HardwareBenchmarkConfig,
) -> EvaluationResult<BTreeMap<String, agentir_runtime_wgpu::RuntimeInput>> {
    if config.input_distribution != "deterministic_zero_v1" {
        return Err(acquisition_error(
            EvaluationErrorCode::EvaluationAcquisitionBenchmarkConfigInvalid,
            "Stage 7C v1 hardware acquisition supports deterministic_zero_v1 inputs",
        ));
    }
    let elements = config
        .tensor_dimensions
        .iter()
        .try_fold(1_u64, |total, dimension| total.checked_mul(*dimension))
        .ok_or_else(acquisition_overflow)?;
    let elements = usize::try_from(elements).map_err(|_| acquisition_overflow())?;
    let mut inputs = BTreeMap::new();
    for layout in &package.manifest.binding_layouts {
        for binding in &layout.storage_bindings {
            if let Some(name) = &binding.external_name {
                inputs.entry(name.clone()).or_insert_with(|| {
                    agentir_runtime_wgpu::RuntimeInput::F32Tensor(vec![0.0; elements])
                });
            }
        }
        for parameter in &layout.parameter_block.entries {
            let value = match parameter.ty {
                BackendParameterType::F32 => agentir_runtime_wgpu::RuntimeInput::F32(0.0),
                BackendParameterType::I32 => agentir_runtime_wgpu::RuntimeInput::I32(0),
                BackendParameterType::U32 => continue,
            };
            inputs.entry(parameter.name.clone()).or_insert(value);
        }
    }
    Ok(inputs)
}

/// Abstract publication store used by production and explicit fixtures.
pub trait MeasurementAcquisitionStore: Clone {
    /// Atomically publishes one complete record and returns its persistent ID/hash.
    fn publish(
        &mut self,
        record: HardwareMeasurementRecord,
    ) -> EvaluationResult<(MeasurementId, String)>;
    /// Resolves one exact completed record without hardware work.
    fn get(&self, id: &MeasurementId) -> Option<&HardwareMeasurementRecord>;

    /// Returns every retained production measurement in deterministic ID order.
    ///
    /// This is the server-owned, zero-device observation boundary used by
    /// Stage 7D publication snapshots and reconciliation.
    fn records(&self) -> Vec<(MeasurementId, HardwareMeasurementRecord)>;
}

impl MeasurementAcquisitionStore for Workspace {
    fn publish(
        &mut self,
        record: HardwareMeasurementRecord,
    ) -> EvaluationResult<(MeasurementId, String)> {
        let id = self
            .measurement_publish(record)
            .map_err(|error| core_acquisition_error(&error))?;
        let hash = self
            .measurement_query(&id)
            .map_err(|error| core_acquisition_error(&error))?
            .measurement_hash
            .to_string();
        Ok((id, hash))
    }

    fn get(&self, id: &MeasurementId) -> Option<&HardwareMeasurementRecord> {
        self.measurement_store().records.get(id)
    }

    fn records(&self) -> Vec<(MeasurementId, HardwareMeasurementRecord)> {
        self.measurement_store()
            .records
            .iter()
            .map(|(id, record)| (id.clone(), record.clone()))
            .collect()
    }
}

/// Explicit test/study-only production-format measurement store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntheticMeasurementAcquisitionStore {
    /// Complete records keyed by deterministic fixture ID.
    pub records: BTreeMap<MeasurementId, HardwareMeasurementRecord>,
    next_id: u64,
}

impl MeasurementAcquisitionStore for SyntheticMeasurementAcquisitionStore {
    fn publish(
        &mut self,
        mut record: HardwareMeasurementRecord,
    ) -> EvaluationResult<(MeasurementId, String)> {
        if record.validation_status != "synthetic_test_data_not_performance_evidence" {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode,
                "fixture store accepts only explicitly synthetic records",
            ));
        }
        record.measurement_hash =
            measurement_hash(&record).map_err(|error| core_acquisition_error(&error))?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(acquisition_overflow)?;
        let id = MeasurementId::new(format!("synthetic-meas-{}", self.next_id));
        let hash = record.measurement_hash.to_string();
        self.records.insert(id.clone(), record);
        Ok((id, hash))
    }

    fn get(&self, id: &MeasurementId) -> Option<&HardwareMeasurementRecord> {
        self.records.get(id)
    }

    fn records(&self) -> Vec<(MeasurementId, HardwareMeasurementRecord)> {
        self.records
            .iter()
            .map(|(id, record)| (id.clone(), record.clone()))
            .collect()
    }
}

impl SyntheticMeasurementAcquisitionStore {
    /// Restores fully verified synthetic records from an evaluation archive.
    pub(crate) fn restore_records(
        &mut self,
        records: impl IntoIterator<Item = (MeasurementId, HardwareMeasurementRecord)>,
    ) -> EvaluationResult<()> {
        let mut restored = BTreeMap::new();
        let mut next_id = 0_u64;
        for (id, record) in records {
            if record.validation_status != "synthetic_test_data_not_performance_evidence"
                || measurement_hash(&record).map_err(|error| core_acquisition_error(&error))?
                    != record.measurement_hash
                || restored.insert(id.clone(), record).is_some()
            {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
                    "archived synthetic measurement is corrupt or duplicated",
                ));
            }
            if let Some(ordinal) = id
                .as_str()
                .strip_prefix("synthetic-meas-")
                .and_then(|value| value.parse::<u64>().ok())
            {
                next_id = next_id.max(ordinal);
            }
        }
        self.records = restored;
        self.next_id = next_id;
        Ok(())
    }
}

impl MeasurementAcquisitionSession {
    /// Starts only after successful server-owned preflight.
    pub fn start<E: MeasurementAcquisitionExecutor>(
        plan: MeasurementAcquisitionPlan,
        catalog: &MeasurementAcquisitionCatalog,
        workspace: Option<&Workspace>,
        executor: &mut E,
    ) -> EvaluationResult<Self> {
        plan.verify()?;
        validate_catalog_against_plan(catalog, &plan)?;
        let preflight = executor.preflight(workspace, catalog, &plan)?;
        if preflight.compiler_build_hash != plan.compiler_build_hash
            || preflight.device_fingerprint_hash.is_empty()
            || preflight.runtime_version.is_empty()
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionPlanInvalid,
                "server-owned preflight returned incompatible anchors",
            ));
        }
        let slots = plan
            .slots()?
            .into_iter()
            .map(|planned| MeasurementAcquisitionSlot {
                version: 1,
                measurement_acquisition_plan_hash: plan.measurement_acquisition_plan_hash.clone(),
                slot_index: planned.slot_index,
                round_index: planned.round_index,
                artifact_hash: planned.artifact_hash,
                target_hash: plan.target_hash.clone(),
                compiler_build_hash: plan.compiler_build_hash.clone(),
                device_fingerprint_hash: preflight.device_fingerprint_hash.clone(),
                runtime_version: preflight.runtime_version.clone(),
                benchmark_config: plan.benchmark_config.clone(),
                status: MeasurementAcquisitionSlotStatus::Pending,
                measurement_id: None,
                measurement_hash: None,
                failure_code: None,
            })
            .collect();
        let mut trace = MeasurementAcquisitionTrace {
            version: 1,
            measurement_acquisition_plan_hash: plan.measurement_acquisition_plan_hash.clone(),
            events: vec![MeasurementAcquisitionTraceEvent {
                sequence: 0,
                kind: "session_started_after_preflight".to_owned(),
                slot_index: None,
                artifact_hash: None,
                measurement_hash: None,
                failure_code: None,
            }],
            measurement_acquisition_trace_hash: String::new(),
        };
        trace.measurement_acquisition_trace_hash = measurement_acquisition_trace_hash(&trace)?;
        Ok(Self {
            version: 1,
            session_id: format!(
                "acquisition-{}",
                &plan.measurement_acquisition_plan_hash[..16]
            ),
            plan,
            preflight,
            slots,
            next_slot: 0,
            status: MeasurementAcquisitionStatus::Running,
            stopping_reason: None,
            cancellation_requested: false,
            work: MeasurementAcquisitionWorkCounters::default(),
            trace,
        })
    }

    /// Advances at most `maximum_slots` complete slots using an atomic staged store/session.
    pub fn advance<S: MeasurementAcquisitionStore, E: MeasurementAcquisitionExecutor>(
        &mut self,
        store: &mut S,
        catalog: &MeasurementAcquisitionCatalog,
        workspace: Option<&Workspace>,
        executor: &mut E,
        maximum_slots: u64,
    ) -> EvaluationResult<MeasurementAcquisitionStatus> {
        if maximum_slots == 0 {
            return Ok(self.status);
        }
        self.verify(store, catalog)?;
        if self.status != MeasurementAcquisitionStatus::Running {
            return Ok(self.status);
        }
        let mut staged_session = self.clone();
        let mut staged_store = store.clone();
        for _ in 0..maximum_slots {
            if staged_session.cancellation_requested {
                staged_session.status = MeasurementAcquisitionStatus::Cancelled;
                staged_session.stopping_reason =
                    Some(MeasurementAcquisitionStoppingReason::CancelledAtSlotBoundary);
                staged_session.work.cancellations =
                    checked_add(staged_session.work.cancellations, 1)?;
                staged_session.push_trace("cancelled_at_slot_boundary", None, None, None)?;
                break;
            }
            let index =
                usize::try_from(staged_session.next_slot).map_err(|_| acquisition_overflow())?;
            if index >= staged_session.slots.len() {
                staged_session.finish_complete()?;
                break;
            }
            let slot = staged_session.slots[index].clone();
            staged_session.work.slots_attempted =
                checked_add(staged_session.work.slots_attempted, 1)?;
            staged_session.work.benchmark_invocations =
                checked_add(staged_session.work.benchmark_invocations, 1)?;
            if staged_session
                .preflight
                .synthetic_test_data_not_performance_evidence
            {
                staged_session.work.synthetic_fixture_invocations =
                    checked_add(staged_session.work.synthetic_fixture_invocations, 1)?;
            }
            match executor.benchmark(
                workspace,
                catalog,
                &staged_session.plan,
                &staged_session.preflight,
                &slot,
            ) {
                Ok((record, device_calls)) => {
                    validate_record(&staged_session, &slot, &record)?;
                    let (measurement_id, measurement_hash) = staged_store.publish(record)?;
                    let target = &mut staged_session.slots[index];
                    target.status = MeasurementAcquisitionSlotStatus::Complete;
                    target.measurement_id = Some(measurement_id);
                    target.measurement_hash = Some(measurement_hash.clone());
                    staged_session.next_slot = checked_add(staged_session.next_slot, 1)?;
                    staged_session.work.slots_completed =
                        checked_add(staged_session.work.slots_completed, 1)?;
                    staged_session.work.published_measurement_records =
                        checked_add(staged_session.work.published_measurement_records, 1)?;
                    staged_session.work.device_calls =
                        checked_add(staged_session.work.device_calls, device_calls)?;
                    staged_session.push_trace(
                        "slot_completed_and_published",
                        Some(slot.slot_index),
                        Some(slot.artifact_hash),
                        Some(measurement_hash),
                    )?;
                }
                Err(error)
                    if matches!(
                        error.code,
                        EvaluationErrorCode::EvaluationAcquisitionDeviceUnavailable
                    ) =>
                {
                    let target = &mut staged_session.slots[index];
                    target.status = MeasurementAcquisitionSlotStatus::DeviceUnavailable;
                    target.failure_code = Some(error.code);
                    staged_session.status = MeasurementAcquisitionStatus::DeviceUnavailable;
                    staged_session.stopping_reason =
                        Some(MeasurementAcquisitionStoppingReason::DeviceUnavailable);
                    staged_session.work.slots_failed =
                        checked_add(staged_session.work.slots_failed, 1)?;
                    staged_session.push_trace(
                        "slot_device_unavailable",
                        Some(slot.slot_index),
                        Some(slot.artifact_hash),
                        None,
                    )?;
                    break;
                }
                Err(error) => {
                    let target = &mut staged_session.slots[index];
                    target.status = MeasurementAcquisitionSlotStatus::BenchmarkFailed;
                    target.failure_code =
                        Some(EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed);
                    staged_session.status = MeasurementAcquisitionStatus::BenchmarkFailed;
                    staged_session.stopping_reason =
                        Some(MeasurementAcquisitionStoppingReason::BenchmarkFailed);
                    staged_session.work.slots_failed =
                        checked_add(staged_session.work.slots_failed, 1)?;
                    staged_session.push_trace(
                        "slot_benchmark_failed",
                        Some(slot.slot_index),
                        Some(slot.artifact_hash),
                        None,
                    )?;
                    if error.code != EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed {
                        // The typed terminal state is retained; the original diagnostic remains
                        // visible through its stable trace classification rather than a sentinel.
                    }
                    break;
                }
            }
            if usize::try_from(staged_session.next_slot).unwrap_or(usize::MAX)
                == staged_session.slots.len()
            {
                staged_session.finish_complete()?;
                break;
            }
        }
        *store = staged_store;
        *self = staged_session;
        Ok(self.status)
    }

    /// Requests cooperative cancellation and observes it immediately at this slot boundary.
    pub fn cancel(&mut self) -> EvaluationResult<MeasurementAcquisitionStatus> {
        if self.status == MeasurementAcquisitionStatus::Running {
            self.cancellation_requested = true;
            self.status = MeasurementAcquisitionStatus::Cancelled;
            self.stopping_reason =
                Some(MeasurementAcquisitionStoppingReason::CancelledAtSlotBoundary);
            self.work.cancellations = checked_add(self.work.cancellations, 1)?;
            self.push_trace("cancelled_at_slot_boundary", None, None, None)?;
        }
        Ok(self.status)
    }

    /// Creates an independently hashed checkpoint between completed slots.
    pub fn checkpoint(&self) -> EvaluationResult<MeasurementAcquisitionCheckpoint> {
        self.checkpoint_with_limits(&MeasurementAcquisitionLimits::default())
    }

    /// Creates a checkpoint under an operational encoded-byte limit.
    pub fn checkpoint_with_limits(
        &self,
        limits: &MeasurementAcquisitionLimits,
    ) -> EvaluationResult<MeasurementAcquisitionCheckpoint> {
        let completed = self.completed_slots();
        let mut checkpoint = MeasurementAcquisitionCheckpoint {
            version: 1,
            measurement_acquisition_plan_hash: self.plan.measurement_acquisition_plan_hash.clone(),
            workspace_id: self.plan.workspace_id.clone(),
            root_anchor_hash: self.plan.root_anchor_hash.clone(),
            device_fingerprint_hash: self.preflight.device_fingerprint_hash.clone(),
            compiler_build_hash: self.preflight.compiler_build_hash.clone(),
            runtime_version: self.preflight.runtime_version.clone(),
            next_slot: self.next_slot,
            completed_slot_indices: completed.iter().map(|slot| slot.slot_index).collect(),
            measurement_ids: completed
                .iter()
                .filter_map(|slot| slot.measurement_id.clone())
                .collect(),
            measurement_hashes: completed
                .iter()
                .filter_map(|slot| slot.measurement_hash.clone())
                .collect(),
            status: self.status,
            session: Box::new(self.clone()),
            measurement_acquisition_checkpoint_hash: String::new(),
        };
        checkpoint.measurement_acquisition_checkpoint_hash =
            measurement_acquisition_checkpoint_hash(&checkpoint)?;
        let encoded_bytes = u64::try_from(
            serde_json::to_vec(&checkpoint)
                .map_err(|error| {
                    acquisition_error(
                        EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                        error.to_string(),
                    )
                })?
                .len(),
        )
        .map_err(|_| acquisition_overflow())?;
        if encoded_bytes > limits.checkpoint_bytes {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionLimitExceeded,
                "measurement acquisition checkpoint exceeds its byte limit",
            )
            .expected_actual(json!(limits.checkpoint_bytes), json!(encoded_bytes)));
        }
        Ok(checkpoint)
    }

    /// Resumes only after verifying every checkpoint/store/catalog anchor without hardware work.
    pub fn resume<S: MeasurementAcquisitionStore>(
        checkpoint: &MeasurementAcquisitionCheckpoint,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        current_preflight: &MeasurementAcquisitionPreflight,
    ) -> EvaluationResult<Self> {
        if checkpoint.measurement_acquisition_checkpoint_hash
            != measurement_acquisition_checkpoint_hash(checkpoint)?
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                "acquisition checkpoint hash is corrupt",
            ));
        }
        let mut session = (*checkpoint.session).clone();
        session.verify(store, catalog)?;
        let completed = session.completed_slots();
        let completed_slot_indices = completed
            .iter()
            .map(|slot| slot.slot_index)
            .collect::<Vec<_>>();
        let measurement_ids = completed
            .iter()
            .filter_map(|slot| slot.measurement_id.clone())
            .collect::<Vec<_>>();
        let measurement_hashes = completed
            .iter()
            .filter_map(|slot| slot.measurement_hash.clone())
            .collect::<Vec<_>>();
        if checkpoint.next_slot != session.next_slot
            || checkpoint.status != session.status
            || checkpoint.completed_slot_indices != completed_slot_indices
            || checkpoint.measurement_ids != measurement_ids
            || checkpoint.measurement_hashes != measurement_hashes
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                "acquisition checkpoint summary differs from its session snapshot",
            ));
        }
        if checkpoint.measurement_acquisition_plan_hash
            != session.plan.measurement_acquisition_plan_hash
            || checkpoint.workspace_id != catalog.workspace_id
            || checkpoint.root_anchor_hash != catalog.root_anchor_hash
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                "acquisition checkpoint device/build/runtime/workspace anchors are stale",
            ));
        }
        if checkpoint.device_fingerprint_hash != current_preflight.device_fingerprint_hash {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionDeviceChanged,
                "device fingerprint changed after the acquisition checkpoint",
            ));
        }
        if checkpoint.compiler_build_hash != current_preflight.compiler_build_hash {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCompilerBuildChanged,
                "compiler build changed after the acquisition checkpoint",
            ));
        }
        if checkpoint.runtime_version != current_preflight.runtime_version {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionRuntimeChanged,
                "runtime changed after the acquisition checkpoint",
            ));
        }
        session.status = MeasurementAcquisitionStatus::Running;
        session.stopping_reason = None;
        session.cancellation_requested = false;
        session.work.resumes = checked_add(session.work.resumes, 1)?;
        session.work.prevented_reruns = checked_add(
            session.work.prevented_reruns,
            u64::try_from(checkpoint.completed_slot_indices.len()).unwrap_or(u64::MAX),
        )?;
        Ok(session)
    }

    /// Records server-observed crash ambiguity without rerunning the current slot.
    pub fn mark_indeterminate_after_crash(&mut self) -> EvaluationResult<()> {
        if self.status != MeasurementAcquisitionStatus::Running {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "only a running acquisition can mark its current slot indeterminate",
            ));
        }
        let index = usize::try_from(self.next_slot).map_err(|_| acquisition_overflow())?;
        let slot = self.slots.get_mut(index).ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "no current acquisition slot exists for crash reconciliation",
            )
        })?;
        slot.status = MeasurementAcquisitionSlotStatus::IndeterminateAfterCrash;
        slot.failure_code = Some(EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash);
        let slot_index = slot.slot_index;
        let artifact_hash = slot.artifact_hash.clone();
        self.status = MeasurementAcquisitionStatus::Failed;
        self.stopping_reason = Some(MeasurementAcquisitionStoppingReason::IndeterminateAfterCrash);
        self.work.slots_indeterminate = checked_add(self.work.slots_indeterminate, 1)?;
        self.push_trace(
            "slot_indeterminate_after_crash",
            Some(slot_index),
            Some(artifact_hash),
            None,
        )
    }

    /// Produces an immutable terminal result; running sessions are rejected.
    pub fn result(&self) -> EvaluationResult<MeasurementAcquisitionResult> {
        let stopping_reason = self.stopping_reason.clone().ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionResultCorrupt,
                "running acquisition has no terminal result",
            )
        })?;
        let completed = self.completed_slots();
        let mut counts = BTreeMap::new();
        for slot in &completed {
            *counts.entry(slot.artifact_hash.clone()).or_insert(0_u64) =
                checked_add(counts.get(&slot.artifact_hash).copied().unwrap_or(0), 1)?;
        }
        let mut result = MeasurementAcquisitionResult {
            version: 1,
            measurement_acquisition_plan_hash: self.plan.measurement_acquisition_plan_hash.clone(),
            status: self.status,
            stopping_reason,
            slots: self.slots.clone(),
            measurement_ids: completed.iter().filter_map(|slot| slot.measurement_id.clone()).collect(),
            measurement_hashes: completed.iter().filter_map(|slot| slot.measurement_hash.clone()).collect(),
            completed_records_per_artifact: counts,
            device_fingerprint_hash: self.preflight.device_fingerprint_hash.clone(),
            compiler_build_hash: self.preflight.compiler_build_hash.clone(),
            runtime_version: self.preflight.runtime_version.clone(),
            benchmark_config: self.plan.benchmark_config.clone(),
            work: self.work.clone(),
            measurement_acquisition_trace_hash: self.trace.measurement_acquisition_trace_hash.clone(),
            provenance: "hardware measurements are non-correctness observations; this result is not a cohort, performance proof, fastest-artifact claim, portability claim, or global-optimality claim".to_owned(),
            measurement_acquisition_result_hash: String::new(),
        };
        result.measurement_acquisition_result_hash = measurement_acquisition_result_hash(&result)?;
        Ok(result)
    }

    /// Offline replay of plan/slot/measurement/hash/work state; executor access is absent.
    pub fn replay<S: MeasurementAcquisitionStore>(
        &self,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
    ) -> EvaluationResult<MeasurementAcquisitionResult> {
        self.verify(store, catalog)?;
        if self.work.replay_hardware_calls != 0 {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionReplayHardwareWork,
                "acquisition replay attempted hardware work",
            ));
        }
        self.result()
    }

    pub(crate) fn verify<S: MeasurementAcquisitionStore>(
        &self,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
    ) -> EvaluationResult<()> {
        self.plan.verify()?;
        validate_catalog_against_plan(catalog, &self.plan)?;
        if self.version != 1
            || self.trace.measurement_acquisition_trace_hash
                != measurement_acquisition_trace_hash(&self.trace)?
            || self.slots.len() != self.plan.slots()?.len()
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionTraceCorrupt,
                "acquisition session trace or slot cardinality is corrupt",
            ));
        }
        let planned = self.plan.slots()?;
        let mut seen = BTreeSet::new();
        let mut completed = 0_u64;
        for (slot, expected) in self.slots.iter().zip(planned) {
            if slot.slot_index != expected.slot_index
                || slot.round_index != expected.round_index
                || slot.artifact_hash != expected.artifact_hash
            {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionSlotOrderMismatch,
                    "acquisition slot order differs from canonical round robin",
                ));
            }
            if slot.status == MeasurementAcquisitionSlotStatus::Complete {
                let id = slot.measurement_id.as_ref().ok_or_else(|| {
                    acquisition_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        "completed slot has no measurement ID",
                    )
                })?;
                let retained = store.get(id).ok_or_else(|| {
                    acquisition_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        "completed measurement is missing",
                    )
                })?;
                let hash = measurement_hash(retained)
                    .map_err(|error| core_acquisition_error(&error))?
                    .to_string();
                if slot.measurement_hash.as_deref() != Some(hash.as_str()) {
                    return Err(acquisition_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        "completed slot measurement hash differs from retained record",
                    ));
                }
                if !seen.insert(hash) {
                    return Err(acquisition_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
                        "duplicate completed measurement hash",
                    ));
                }
                completed = checked_add(completed, 1)?;
            } else if slot.measurement_id.is_some() || slot.measurement_hash.is_some() {
                return Err(acquisition_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "non-complete slot retains a measurement anchor",
                ));
            }
        }
        if self.next_slot != completed {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionSlotOrderMismatch,
                "next slot differs from the completed canonical prefix",
            ));
        }
        Ok(())
    }

    /// Returns the exact current pending slot for Stage 7D preparation.
    pub(crate) fn recovery_pending_slot(&self) -> EvaluationResult<&MeasurementAcquisitionSlot> {
        if self.status != MeasurementAcquisitionStatus::Running {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "measurement acquisition session is not running",
            ));
        }
        let index = usize::try_from(self.next_slot).map_err(|_| acquisition_overflow())?;
        let slot = self.slots.get(index).ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionSlotOrderMismatch,
                "measurement acquisition has no current pending slot",
            )
        })?;
        if slot.status != MeasurementAcquisitionSlotStatus::Pending
            || slot.measurement_id.is_some()
            || slot.measurement_hash.is_some()
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionSlotOrderMismatch,
                "current measurement acquisition slot is not pristine and pending",
            ));
        }
        Ok(slot)
    }

    /// Attaches one server-verified publication to the current Stage 7C slot.
    ///
    /// The resulting Stage 7C trace/result bytes are identical to an ordinary
    /// successful `advance` for the same record. Recovery-specific provenance
    /// remains in the separate Stage 7D journal.
    pub(crate) fn attach_recovered_measurement(
        &mut self,
        measurement_id: MeasurementId,
        measurement_hash: String,
        device_calls: u64,
    ) -> EvaluationResult<()> {
        let index = usize::try_from(self.next_slot).map_err(|_| acquisition_overflow())?;
        let slot = self.recovery_pending_slot()?.clone();
        self.work.slots_attempted = checked_add(self.work.slots_attempted, 1)?;
        self.work.benchmark_invocations = checked_add(self.work.benchmark_invocations, 1)?;
        if self.preflight.synthetic_test_data_not_performance_evidence {
            self.work.synthetic_fixture_invocations =
                checked_add(self.work.synthetic_fixture_invocations, 1)?;
        }
        let target = &mut self.slots[index];
        target.status = MeasurementAcquisitionSlotStatus::Complete;
        target.measurement_id = Some(measurement_id);
        target.measurement_hash = Some(measurement_hash.clone());
        self.next_slot = checked_add(self.next_slot, 1)?;
        self.work.slots_completed = checked_add(self.work.slots_completed, 1)?;
        self.work.published_measurement_records =
            checked_add(self.work.published_measurement_records, 1)?;
        self.work.device_calls = checked_add(self.work.device_calls, device_calls)?;
        self.push_trace(
            "slot_completed_and_published",
            Some(slot.slot_index),
            Some(slot.artifact_hash),
            Some(measurement_hash),
        )?;
        if usize::try_from(self.next_slot).unwrap_or(usize::MAX) == self.slots.len() {
            self.finish_complete()?;
        }
        Ok(())
    }

    fn completed_slots(&self) -> Vec<&MeasurementAcquisitionSlot> {
        self.slots
            .iter()
            .filter(|slot| slot.status == MeasurementAcquisitionSlotStatus::Complete)
            .collect()
    }

    fn finish_complete(&mut self) -> EvaluationResult<()> {
        self.status = MeasurementAcquisitionStatus::Complete;
        self.stopping_reason = Some(MeasurementAcquisitionStoppingReason::AllSlotsComplete);
        self.push_trace("all_slots_complete", None, None, None)
    }

    fn push_trace(
        &mut self,
        kind: &str,
        slot_index: Option<u64>,
        artifact_hash: Option<String>,
        measurement_hash: Option<String>,
    ) -> EvaluationResult<()> {
        let sequence =
            u64::try_from(self.trace.events.len()).map_err(|_| acquisition_overflow())?;
        let failure_code = match kind {
            "slot_benchmark_failed" => {
                Some(EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed)
            }
            "slot_device_unavailable" => {
                Some(EvaluationErrorCode::EvaluationAcquisitionDeviceUnavailable)
            }
            _ => None,
        };
        self.trace.events.push(MeasurementAcquisitionTraceEvent {
            sequence,
            kind: kind.to_owned(),
            slot_index,
            artifact_hash,
            measurement_hash,
            failure_code,
        });
        self.trace.measurement_acquisition_trace_hash =
            measurement_acquisition_trace_hash(&self.trace)?;
        Ok(())
    }
}

fn validate_catalog_against_plan(
    catalog: &MeasurementAcquisitionCatalog,
    plan: &MeasurementAcquisitionPlan,
) -> EvaluationResult<()> {
    if catalog.workspace_id != plan.workspace_id
        || catalog.root_anchor_hash != plan.root_anchor_hash
    {
        return Err(acquisition_error(
            EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
            "production workspace/root anchor changed",
        ));
    }
    for hash in &plan.artifact_hashes {
        let artifact = catalog.artifacts.get(hash).ok_or_else(|| {
            acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                "planned artifact is no longer retained",
            )
        })?;
        if !artifact.offline_valid
            || artifact.spec_hash != plan.spec_hash
            || artifact.target_hash != plan.target_hash
            || artifact.compiler_build_hash != plan.compiler_build_hash
        {
            return Err(acquisition_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                "planned artifact status or anchors changed",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_record(
    session: &MeasurementAcquisitionSession,
    slot: &MeasurementAcquisitionSlot,
    record: &HardwareMeasurementRecord,
) -> EvaluationResult<()> {
    let accepted_status = match session.plan.validation_policy {
        MeasurementValidationPolicy::HardwareExecutedV1 => "offline_validated_and_device_executed",
        MeasurementValidationPolicy::SyntheticFixtureV1 => {
            "synthetic_test_data_not_performance_evidence"
        }
    };
    if record.artifact_hash.as_str() != slot.artifact_hash
        || record.target_hash.as_str() != session.plan.target_hash
        || record.compiler_build_hash.as_str() != session.preflight.compiler_build_hash
        || record.device_fingerprint_hash.as_str() != session.preflight.device_fingerprint_hash
        || record.runtime_version != session.preflight.runtime_version
        || record.config != session.plan.benchmark_config
        || record.validation_status != accepted_status
        || record.measurement_hash.as_str() != "pending"
    {
        return Err(acquisition_error(
            EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed,
            "executor returned incompatible or client-shaped measurement data",
        ));
    }
    Ok(())
}

/// Computes the independent checkpoint hash.
pub fn measurement_acquisition_checkpoint_hash(
    checkpoint: &MeasurementAcquisitionCheckpoint,
) -> EvaluationResult<String> {
    domain_hash_cleared(
        MEASUREMENT_ACQUISITION_CHECKPOINT_HASH_DOMAIN,
        checkpoint,
        |model| {
            model.measurement_acquisition_checkpoint_hash.clear();
        },
    )
}

/// Computes the independent trace hash.
pub fn measurement_acquisition_trace_hash(
    trace: &MeasurementAcquisitionTrace,
) -> EvaluationResult<String> {
    domain_hash_cleared(MEASUREMENT_ACQUISITION_TRACE_HASH_DOMAIN, trace, |model| {
        model.measurement_acquisition_trace_hash.clear();
    })
}

/// Computes the independent result hash.
pub fn measurement_acquisition_result_hash(
    result: &MeasurementAcquisitionResult,
) -> EvaluationResult<String> {
    let mut model = result.clone();
    model.measurement_acquisition_result_hash.clear();
    model.work = MeasurementAcquisitionWorkCounters::default();
    crate::hashing::domain_hash(MEASUREMENT_ACQUISITION_RESULT_HASH_DOMAIN, &model)
}

fn checked_add(left: u64, right: u64) -> EvaluationResult<u64> {
    left.checked_add(right).ok_or_else(acquisition_overflow)
}

fn core_acquisition_error(error: &agentir_core::AgentError) -> EvaluationDiagnostic {
    let code = match error.code {
        ErrorCode::DeviceUnavailable | ErrorCode::DeviceCapabilityUnsupported => {
            EvaluationErrorCode::EvaluationAcquisitionDeviceUnavailable
        }
        ErrorCode::ResourceLimitExceeded | ErrorCode::BenchmarkLimitExceeded => {
            EvaluationErrorCode::EvaluationAcquisitionLimitExceeded
        }
        _ => EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed,
    };
    acquisition_error(code, error.to_string())
}
