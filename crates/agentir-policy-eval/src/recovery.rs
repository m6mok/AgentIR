//! Stage 7D durable acquisition recovery and reconciliation contracts.
//!
//! Recovery is an evaluation-only, single-writer protocol around the frozen
//! Stage 7C acquisition session and production measurement store. Durable
//! preparation precedes hardware work. After a crash, reconciliation observes
//! already-published production records without opening a device and never
//! silently repeats an indeterminate slot.

use crate::{
    acquisition::{
        MeasurementAcquisitionCatalog, MeasurementAcquisitionExecutor,
        MeasurementAcquisitionSession, MeasurementAcquisitionSlot, MeasurementAcquisitionStatus,
        MeasurementAcquisitionStore, validate_record,
    },
    hashing::{domain_hash, domain_hash_cleared},
    measured::MeasurementValidationPolicy,
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult},
};
use agentir_core::{
    backend::measurement_hash,
    backend_ir::{HardwareBenchmarkConfig, HardwareMeasurementRecord},
    ids::MeasurementId,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Recovery-journal hash domain.
pub const MEASUREMENT_ACQUISITION_RECOVERY_JOURNAL_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_recovery_journal.v1\0";
/// Prepared-slot hash domain.
pub const MEASUREMENT_ACQUISITION_PREPARED_SLOT_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_prepared_slot.v1\0";
/// Reconciliation-result hash domain.
pub const MEASUREMENT_ACQUISITION_RECONCILIATION_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measurement_acquisition_reconciliation.v1\0";

/// Operational Stage 7D limits excluded from semantic identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementAcquisitionRecoveryLimits {
    /// Maximum retained recovery journals.
    pub retained_journals: u64,
    /// Maximum prepared-slot records in one journal.
    pub prepared_slots: u64,
    /// Maximum measurement anchors in one publication snapshot.
    pub publication_snapshot_records: u64,
    /// Maximum new records inspected by one reconciliation.
    pub reconciliation_candidates: u64,
    /// Maximum explicitly authorized retry attempts per slot.
    pub retry_attempts: u64,
    /// Maximum semantic recovery trace events in one journal.
    pub recovery_trace_events: u64,
    /// Maximum encoded recovery checkpoint bytes.
    pub checkpoint_bytes: u64,
    /// Maximum records/events inspected by recovery replay.
    pub replay_work: u64,
    /// Maximum encoded evaluation archive v7 bytes.
    pub archive_v7_bytes: u64,
}

impl Default for MeasurementAcquisitionRecoveryLimits {
    fn default() -> Self {
        Self {
            retained_journals: 1_024,
            prepared_slots: 10_000,
            publication_snapshot_records: 1_000_000,
            reconciliation_candidates: 10_000,
            retry_attempts: 32,
            recovery_trace_events: 100_000,
            checkpoint_bytes: 256 * 1024 * 1024,
            replay_work: 1_000_000,
            archive_v7_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Server-owned preparation state for one hardware attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionPreparationStatus {
    /// Durable preparation exists and execution has not been accepted.
    Prepared,
    /// A separate retry authorization permits this exact attempt.
    RetryAuthorized,
    /// Hardware may have executed; automatic execution is forbidden.
    IndeterminateAfterCrash,
    /// One compatible publication was reconciled into the Stage 7C slot.
    Reconciled,
    /// The operator explicitly abandoned the prepared slot.
    Abandoned,
}

/// Durable recovery lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    /// One durable prepared attempt exists and may be explicitly executed.
    Prepared,
    /// Hardware/publication completion is uncertain and reconciliation is required.
    IndeterminateAfterCrash,
    /// Reconciliation proved that no post-prepare publication currently exists.
    NoPublicationObserved,
    /// A separate command authorized one new attempt.
    RetryAuthorized,
    /// Exactly one compatible publication was attached without a benchmark.
    Reconciled,
    /// Multiple compatible publications require external resolution.
    Ambiguous,
    /// An incompatible publication or changed anchor blocks recovery.
    Blocked,
    /// The prepared slot was explicitly abandoned.
    Abandoned,
    /// Stage 7C already records this slot as complete.
    Complete,
}

/// Typed result of server-owned, zero-device reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationOutcome {
    /// No record appeared after the preparation snapshot.
    NoPublicationObserved,
    /// Exactly one compatible new record appeared.
    ExactlyOneCompatiblePublication,
    /// More than one compatible new record appeared.
    MultipleCompatiblePublications,
    /// A new record appeared but violated a frozen slot anchor.
    IncompatiblePublicationObserved,
    /// Workspace or root identity changed.
    WorkspaceChanged,
    /// Device identity changed.
    DeviceChanged,
    /// Compiler build identity changed.
    BuildChanged,
    /// Runtime identity changed.
    RuntimeChanged,
    /// Journal, snapshot, or referenced record was corrupt.
    CorruptRecoveryJournal,
    /// Exactly one record was attached to the Stage 7C slot.
    Reconciled,
    /// A new retry attempt was explicitly authorized.
    RetryAuthorized,
    /// Recovery was explicitly abandoned.
    Abandoned,
}

/// Safe fault-injection boundaries used by tests and readiness studies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionRecoveryFaultBoundary {
    /// Stop after durable prepare and before any benchmark call.
    BeforeBenchmark,
    /// Stop after benchmark return and before production publication.
    AfterBenchmarkBeforePublication,
    /// Publish a complete record, then stop before updating Stage 7C progress.
    AfterPublicationBeforeCheckpoint,
    /// Complete publication, Stage 7C progress, and recovery checkpoint first.
    AfterCheckpoint,
}

/// One exact production measurement present at a publication boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPublicationAnchor {
    /// Compiler-assigned production measurement ID.
    pub measurement_id: MeasurementId,
    /// Exact verified production measurement hash.
    pub measurement_hash: String,
}

/// Server-owned exact publication snapshot v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPublicationSnapshot {
    /// Snapshot schema version.
    pub version: u32,
    /// Exact records visible before hardware authorization.
    pub records: Vec<MeasurementAcquisitionPublicationAnchor>,
}

/// Server-owned current recovery anchors; obtaining them performs no benchmark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryAnchors {
    /// Exact production workspace identity.
    pub workspace_id: String,
    /// Exact immutable root identity.
    pub root_anchor_hash: String,
    /// Exact device fingerprint retained by the server.
    pub device_fingerprint_hash: String,
    /// Exact compiler build retained by the server.
    pub compiler_build_hash: String,
    /// Exact runtime implementation retained by the server.
    pub runtime_version: String,
}

impl MeasurementAcquisitionRecoveryAnchors {
    /// Derives the server-owned anchors already retained by a Stage 7C session.
    #[must_use]
    pub fn from_session(session: &MeasurementAcquisitionSession) -> Self {
        Self {
            workspace_id: session.plan.workspace_id.clone(),
            root_anchor_hash: session.plan.root_anchor_hash.clone(),
            device_fingerprint_hash: session.preflight.device_fingerprint_hash.clone(),
            compiler_build_hash: session.preflight.compiler_build_hash.clone(),
            runtime_version: session.preflight.runtime_version.clone(),
        }
    }
}

/// Durable prepared-slot record v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionPreparedSlot {
    /// Prepared-slot schema version.
    pub version: u32,
    /// Frozen Stage 7C plan hash.
    pub measurement_acquisition_plan_hash: String,
    /// Frozen Stage 7C session identity.
    pub session_id: String,
    /// Canonical slot index.
    pub slot_index: u64,
    /// Canonical record round.
    pub round_index: u64,
    /// Exact compiler-owned artifact hash.
    pub artifact_hash: String,
    /// Exact production workspace anchor.
    pub workspace_id: String,
    /// Exact immutable root anchor.
    pub root_anchor_hash: String,
    /// Exact target anchor.
    pub target_hash: String,
    /// Exact compiler build anchor.
    pub compiler_build_hash: String,
    /// Exact device fingerprint anchor.
    pub device_fingerprint_hash: String,
    /// Exact runtime anchor.
    pub runtime_version: String,
    /// Exact benchmark configuration.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Exact validation policy inherited from the Stage 7C plan.
    pub validation_policy: MeasurementValidationPolicy,
    /// Server-assigned attempt identity.
    pub attempt_id: String,
    /// Exact production publication boundary before hardware work.
    pub publication_snapshot: MeasurementAcquisitionPublicationSnapshot,
    /// Durable preparation lifecycle state.
    pub preparation_status: MeasurementAcquisitionPreparationStatus,
    /// Independent prepared-slot hash.
    pub measurement_acquisition_prepared_slot_hash: String,
}

/// Deterministic non-correctness recovery work counters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryWorkCounters {
    /// Publication records captured in preparation snapshots.
    pub snapshot_records: u64,
    /// Durable preparations accepted.
    pub prepared_slots: u64,
    /// Explicit execute commands accepted.
    pub execute_commands: u64,
    /// Benchmark invocations made only by explicit execute.
    pub benchmark_invocations: u64,
    /// Concrete executor device calls returned by explicit execute.
    pub device_calls: u64,
    /// Production records published by recovery execute.
    pub publications: u64,
    /// Reconciliation commands accepted.
    pub reconciliation_attempts: u64,
    /// Post-boundary production records inspected.
    pub reconciliation_candidates: u64,
    /// Compatible records attached without hardware work.
    pub reconciled_publications: u64,
    /// Explicit retry authorizations.
    pub retry_authorizations: u64,
    /// Explicit abandon commands.
    pub abandonments: u64,
    /// Recovery checkpoints encoded.
    pub checkpoints: u64,
    /// Recovery replay commands.
    pub replays: u64,
    /// Hardware calls during replay; valid replay requires zero.
    pub replay_hardware_calls: u64,
    /// Automatic reruns prevented by the journal.
    pub prevented_automatic_reruns: u64,
}

/// One deterministic recovery lifecycle event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryTraceEvent {
    /// Zero-based contiguous event sequence.
    pub sequence: u64,
    /// Stable event kind.
    pub kind: String,
    /// Exact attempt identity when applicable.
    pub attempt_id: Option<String>,
    /// Canonical slot index.
    pub slot_index: u64,
    /// Typed observed outcome when applicable.
    pub outcome: Option<ReconciliationOutcome>,
}

/// One immutable retry authorization record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRetryAuthorization {
    /// Attempt that reconciliation proved had no publication.
    pub prior_attempt_id: String,
    /// Newly authorized server-assigned attempt.
    pub authorized_attempt_id: String,
    /// Exact reconciliation result authorizing the retry.
    pub reconciliation_hash: String,
}

/// Reconciliation result v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionReconciliationResult {
    /// Reconciliation schema version.
    pub version: u32,
    /// Deterministic result identity local to the journal.
    pub reconciliation_id: String,
    /// Exact prepared-slot anchor.
    pub measurement_acquisition_prepared_slot_hash: String,
    /// Server-assigned attempt identity.
    pub attempt_id: String,
    /// Classification before any Stage 7C attachment.
    pub observed_outcome: ReconciliationOutcome,
    /// Final outcome after the atomic reconciliation action.
    pub outcome: ReconciliationOutcome,
    /// Exact compatible candidate IDs observed after the snapshot.
    pub compatible_measurement_ids: Vec<MeasurementId>,
    /// Exact compatible candidate hashes observed after the snapshot.
    pub compatible_measurement_hashes: Vec<String>,
    /// Accepted production measurement ID only after reconciliation.
    pub accepted_measurement_id: Option<MeasurementId>,
    /// Accepted production measurement hash only after reconciliation.
    pub accepted_measurement_hash: Option<String>,
    /// Deterministic non-semantic work for this attempt.
    pub work: MeasurementAcquisitionRecoveryWorkCounters,
    /// Independent reconciliation hash.
    pub measurement_acquisition_reconciliation_hash: String,
}

/// Durable single-writer recovery journal v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryJournal {
    /// Journal schema version.
    pub version: u32,
    /// Deterministic journal identity.
    pub recovery_journal_id: String,
    /// Frozen Stage 7C plan anchor.
    pub measurement_acquisition_plan_hash: String,
    /// Frozen Stage 7C session identity.
    pub session_id: String,
    /// Exact production workspace anchor.
    pub workspace_id: String,
    /// Exact immutable root anchor.
    pub root_anchor_hash: String,
    /// Canonical slot protected by this journal.
    pub slot_index: u64,
    /// Current recovery lifecycle status.
    pub status: RecoveryStatus,
    /// Ordered prepared attempts; retry never mutates an older attempt identity.
    pub prepared_slots: Vec<MeasurementAcquisitionPreparedSlot>,
    /// Ordered zero-device reconciliation results.
    pub reconciliation_results: Vec<MeasurementAcquisitionReconciliationResult>,
    /// Ordered explicit retry authorizations.
    pub retry_authorizations: Vec<MeasurementAcquisitionRetryAuthorization>,
    /// Ordered semantic recovery trace.
    pub trace: Vec<MeasurementAcquisitionRecoveryTraceEvent>,
    /// Deterministic non-semantic work accounting.
    pub work: MeasurementAcquisitionRecoveryWorkCounters,
    /// Per-attempt executor device-call count, excluded from journal identity.
    pub attempt_device_calls: BTreeMap<String, u64>,
    /// Independent recovery-journal hash.
    pub measurement_acquisition_recovery_journal_hash: String,
}

/// Durable recovery checkpoint v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryCheckpoint {
    /// Checkpoint schema version.
    pub version: u32,
    /// Exact journal snapshot.
    pub journal: Box<MeasurementAcquisitionRecoveryJournal>,
    /// Exact Stage 7C session snapshot paired with the recovery journal.
    pub session: Box<MeasurementAcquisitionSession>,
    /// Hash of the exact journal snapshot.
    pub measurement_acquisition_recovery_journal_hash: String,
    /// Independently checked checkpoint hash under the journal domain.
    pub measurement_acquisition_recovery_checkpoint_hash: String,
}

/// Recovery result/status projection used by protocol and replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementAcquisitionRecoveryResult {
    /// Exact journal identity.
    pub recovery_journal_id: String,
    /// Current recovery status.
    pub status: RecoveryStatus,
    /// Canonical protected slot.
    pub slot_index: u64,
    /// Latest reconciliation outcome when present.
    pub latest_outcome: Option<ReconciliationOutcome>,
    /// Exact accepted production measurement ID when reconciled.
    pub accepted_measurement_id: Option<MeasurementId>,
    /// Exact accepted production measurement hash when reconciled.
    pub accepted_measurement_hash: Option<String>,
    /// Exact current journal hash.
    pub measurement_acquisition_recovery_journal_hash: String,
    /// Replay/device work contract.
    pub work: MeasurementAcquisitionRecoveryWorkCounters,
}

/// Stage 7D records attached atomically to evaluation archive v7.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeasurementAcquisitionRecoveryArchiveBundle {
    /// Durable recovery journals.
    pub journals: Vec<MeasurementAcquisitionRecoveryJournal>,
    /// Durable recovery checkpoints.
    pub checkpoints: Vec<MeasurementAcquisitionRecoveryCheckpoint>,
    /// Complete production-format records referenced by recovery history.
    pub records: Vec<crate::measured::MeasurementCohortRecord>,
    /// Explicit zero-device replay status by journal hash.
    pub replay_statuses: BTreeMap<String, bool>,
}

/// Computes the prepared-slot hash.
pub fn measurement_acquisition_prepared_slot_hash(
    prepared: &MeasurementAcquisitionPreparedSlot,
) -> EvaluationResult<String> {
    domain_hash_cleared(
        MEASUREMENT_ACQUISITION_PREPARED_SLOT_HASH_DOMAIN,
        prepared,
        |model| model.measurement_acquisition_prepared_slot_hash.clear(),
    )
}

/// Computes the reconciliation-result hash with work counters excluded.
pub fn measurement_acquisition_reconciliation_hash(
    result: &MeasurementAcquisitionReconciliationResult,
) -> EvaluationResult<String> {
    let mut model = result.clone();
    model.measurement_acquisition_reconciliation_hash.clear();
    model.work = MeasurementAcquisitionRecoveryWorkCounters::default();
    domain_hash(MEASUREMENT_ACQUISITION_RECONCILIATION_HASH_DOMAIN, &model)
}

/// Computes the recovery-journal hash with operational work excluded.
pub fn measurement_acquisition_recovery_journal_hash(
    journal: &MeasurementAcquisitionRecoveryJournal,
) -> EvaluationResult<String> {
    let mut model = journal.clone();
    model.measurement_acquisition_recovery_journal_hash.clear();
    model.work = MeasurementAcquisitionRecoveryWorkCounters::default();
    model.attempt_device_calls.clear();
    for result in &mut model.reconciliation_results {
        result.work = MeasurementAcquisitionRecoveryWorkCounters::default();
    }
    domain_hash(MEASUREMENT_ACQUISITION_RECOVERY_JOURNAL_HASH_DOMAIN, &model)
}

/// Computes the recovery-checkpoint hash.
pub fn measurement_acquisition_recovery_checkpoint_hash(
    checkpoint: &MeasurementAcquisitionRecoveryCheckpoint,
) -> EvaluationResult<String> {
    domain_hash_cleared(
        MEASUREMENT_ACQUISITION_RECOVERY_JOURNAL_HASH_DOMAIN,
        checkpoint,
        |model| {
            model
                .measurement_acquisition_recovery_checkpoint_hash
                .clear()
        },
    )
}

fn recovery_error(code: EvaluationErrorCode, message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}

fn overflow() -> EvaluationDiagnostic {
    recovery_error(
        EvaluationErrorCode::EvaluationAcquisitionCounterOverflow,
        "checked Stage 7D recovery counter overflow",
    )
}

fn add(left: u64, right: u64) -> EvaluationResult<u64> {
    left.checked_add(right).ok_or_else(overflow)
}

fn count(value: usize) -> EvaluationResult<u64> {
    u64::try_from(value).map_err(|_| overflow())
}

fn limit(actual: u64, maximum: u64, resource: &str) -> EvaluationResult<()> {
    if actual > maximum {
        return Err(recovery_error(
            EvaluationErrorCode::EvaluationAcquisitionLimitExceeded,
            format!("Stage 7D recovery resource `{resource}` exceeded"),
        )
        .expected_actual(json!(maximum), json!(actual)));
    }
    Ok(())
}

fn publication_snapshot<S: MeasurementAcquisitionStore>(
    store: &S,
    limits: &MeasurementAcquisitionRecoveryLimits,
) -> EvaluationResult<MeasurementAcquisitionPublicationSnapshot> {
    let records = store.records();
    limit(
        count(records.len())?,
        limits.publication_snapshot_records,
        "publication_snapshot_records",
    )?;
    let mut anchors = Vec::with_capacity(records.len());
    for (measurement_id, record) in records {
        let calculated = measurement_hash(&record)
            .map_err(|error| {
                recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    error.to_string(),
                )
            })?
            .to_string();
        if calculated != record.measurement_hash.to_string() {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                "production measurement store contains a corrupt record",
            ));
        }
        anchors.push(MeasurementAcquisitionPublicationAnchor {
            measurement_id,
            measurement_hash: calculated,
        });
    }
    anchors.sort();
    if anchors.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(recovery_error(
            EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
            "production measurement snapshot contains a duplicate ID/hash anchor",
        ));
    }
    Ok(MeasurementAcquisitionPublicationSnapshot {
        version: 1,
        records: anchors,
    })
}

fn prepared_from_slot(
    session: &MeasurementAcquisitionSession,
    slot: &MeasurementAcquisitionSlot,
    snapshot: MeasurementAcquisitionPublicationSnapshot,
    attempt_ordinal: u64,
    preparation_status: MeasurementAcquisitionPreparationStatus,
) -> EvaluationResult<MeasurementAcquisitionPreparedSlot> {
    let mut prepared = MeasurementAcquisitionPreparedSlot {
        version: 1,
        measurement_acquisition_plan_hash: session.plan.measurement_acquisition_plan_hash.clone(),
        session_id: session.session_id.clone(),
        slot_index: slot.slot_index,
        round_index: slot.round_index,
        artifact_hash: slot.artifact_hash.clone(),
        workspace_id: session.plan.workspace_id.clone(),
        root_anchor_hash: session.plan.root_anchor_hash.clone(),
        target_hash: slot.target_hash.clone(),
        compiler_build_hash: slot.compiler_build_hash.clone(),
        device_fingerprint_hash: slot.device_fingerprint_hash.clone(),
        runtime_version: slot.runtime_version.clone(),
        benchmark_config: slot.benchmark_config.clone(),
        validation_policy: session.plan.validation_policy,
        attempt_id: format!(
            "{}-slot-{}-attempt-{}",
            session.session_id, slot.slot_index, attempt_ordinal
        ),
        publication_snapshot: snapshot,
        preparation_status,
        measurement_acquisition_prepared_slot_hash: String::new(),
    };
    prepared.measurement_acquisition_prepared_slot_hash =
        measurement_acquisition_prepared_slot_hash(&prepared)?;
    Ok(prepared)
}

impl MeasurementAcquisitionPreparedSlot {
    /// Fully verifies the prepared-slot hash and canonical snapshot.
    pub fn verify(&self) -> EvaluationResult<()> {
        if self.version != 1
            || self.attempt_id.is_empty()
            || self.publication_snapshot.version != 1
            || self
                .publication_snapshot
                .records
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.measurement_acquisition_prepared_slot_hash
                != measurement_acquisition_prepared_slot_hash(self)?
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "prepared acquisition slot is corrupt or non-canonical",
            ));
        }
        Ok(())
    }
}

impl MeasurementAcquisitionRecoveryJournal {
    /// Durably prepares the exact current Stage 7C slot without hardware work.
    pub fn prepare<S: MeasurementAcquisitionStore>(
        session: &MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<Self> {
        session.verify(store, catalog)?;
        let slot = session.recovery_pending_slot()?;
        limit(1, limits.prepared_slots, "prepared_slots")?;
        let snapshot = publication_snapshot(store, limits)?;
        let prepared = prepared_from_slot(
            session,
            slot,
            snapshot,
            1,
            MeasurementAcquisitionPreparationStatus::Prepared,
        )?;
        let mut journal = Self {
            version: 1,
            recovery_journal_id: format!(
                "recovery-{}-slot-{}",
                session.session_id, slot.slot_index
            ),
            measurement_acquisition_plan_hash: session
                .plan
                .measurement_acquisition_plan_hash
                .clone(),
            session_id: session.session_id.clone(),
            workspace_id: session.plan.workspace_id.clone(),
            root_anchor_hash: session.plan.root_anchor_hash.clone(),
            slot_index: slot.slot_index,
            status: RecoveryStatus::Prepared,
            prepared_slots: vec![prepared.clone()],
            reconciliation_results: Vec::new(),
            retry_authorizations: Vec::new(),
            trace: vec![MeasurementAcquisitionRecoveryTraceEvent {
                sequence: 0,
                kind: "slot_durably_prepared_before_hardware".to_owned(),
                attempt_id: Some(prepared.attempt_id),
                slot_index: slot.slot_index,
                outcome: None,
            }],
            work: MeasurementAcquisitionRecoveryWorkCounters {
                snapshot_records: count(prepared.publication_snapshot.records.len())?,
                prepared_slots: 1,
                ..MeasurementAcquisitionRecoveryWorkCounters::default()
            },
            attempt_device_calls: BTreeMap::new(),
            measurement_acquisition_recovery_journal_hash: String::new(),
        };
        journal.refresh_hash()?;
        journal.verify(session, store, catalog, limits)?;
        Ok(journal)
    }

    /// Returns the latest immutable prepared attempt.
    pub fn current_prepared_slot(&self) -> EvaluationResult<&MeasurementAcquisitionPreparedSlot> {
        self.prepared_slots.last().ok_or_else(|| {
            recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "recovery journal has no prepared slot",
            )
        })
    }

    /// Verifies journal/prepared/reconciliation hashes and frozen Stage 7C anchors.
    pub fn verify<S: MeasurementAcquisitionStore>(
        &self,
        session: &MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<()> {
        if self.version != 1
            || self.recovery_journal_id.is_empty()
            || self.measurement_acquisition_recovery_journal_hash
                != measurement_acquisition_recovery_journal_hash(self)?
            || self.trace.iter().enumerate().any(|(index, event)| {
                event.sequence != u64::try_from(index).unwrap_or(u64::MAX)
                    || event.slot_index != self.slot_index
            })
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "recovery journal hash or trace is corrupt",
            ));
        }
        limit(
            count(self.prepared_slots.len())?,
            limits.prepared_slots,
            "prepared_slots",
        )?;
        limit(
            count(self.trace.len())?,
            limits.recovery_trace_events,
            "recovery_trace_events",
        )?;
        limit(
            count(self.retry_authorizations.len())?,
            limits.retry_attempts,
            "retry_attempts",
        )?;
        if self.measurement_acquisition_plan_hash != session.plan.measurement_acquisition_plan_hash
            || self.session_id != session.session_id
            || self.workspace_id != session.plan.workspace_id
            || self.root_anchor_hash != session.plan.root_anchor_hash
            || self.slot_index != session.next_slot
                && session
                    .slots
                    .get(usize::try_from(self.slot_index).unwrap_or(usize::MAX))
                    .is_none_or(|slot| {
                        slot.status
                            != crate::acquisition::MeasurementAcquisitionSlotStatus::Complete
                    })
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                "recovery journal differs from its Stage 7C session anchor",
            ));
        }
        if catalog.workspace_id != self.workspace_id
            || catalog.root_anchor_hash != self.root_anchor_hash
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                "recovery journal differs from the production workspace/root anchor",
            ));
        }
        let mut attempt_ids = BTreeSet::new();
        for prepared in &self.prepared_slots {
            prepared.verify()?;
            limit(
                count(prepared.publication_snapshot.records.len())?,
                limits.publication_snapshot_records,
                "publication_snapshot_records",
            )?;
            if !attempt_ids.insert(prepared.attempt_id.as_str())
                || prepared.measurement_acquisition_plan_hash
                    != self.measurement_acquisition_plan_hash
                || prepared.session_id != self.session_id
                || prepared.slot_index != self.slot_index
                || prepared.workspace_id != self.workspace_id
                || prepared.root_anchor_hash != self.root_anchor_hash
            {
                return Err(recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                    "prepared slot is duplicated or has stale journal anchors",
                ));
            }
        }
        let prepared_hashes = self
            .prepared_slots
            .iter()
            .map(|prepared| prepared.measurement_acquisition_prepared_slot_hash.as_str())
            .collect::<BTreeSet<_>>();
        let mut reconciliation_ids = BTreeSet::new();
        for result in &self.reconciliation_results {
            if result.version != 1
                || !reconciliation_ids.insert(result.reconciliation_id.as_str())
                || !prepared_hashes
                    .contains(result.measurement_acquisition_prepared_slot_hash.as_str())
                || result.measurement_acquisition_reconciliation_hash
                    != measurement_acquisition_reconciliation_hash(result)?
            {
                return Err(recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                    "reconciliation result is corrupt, duplicated, or unanchored",
                ));
            }
        }
        // Snapshot anchors must still resolve exactly. New records are allowed.
        let retained = store
            .records()
            .into_iter()
            .map(|(id, record)| (id, record.measurement_hash.to_string()))
            .collect::<BTreeMap<_, _>>();
        for prepared in &self.prepared_slots {
            for anchor in &prepared.publication_snapshot.records {
                if retained.get(&anchor.measurement_id) != Some(&anchor.measurement_hash) {
                    return Err(recovery_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        "prepared publication snapshot no longer resolves exactly",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Explicitly executes the latest prepared attempt.
    ///
    /// Only this method accepts an executor. The three recovery observation,
    /// status, checkpoint, and replay paths have no executor parameter.
    pub fn execute<S, E>(
        &mut self,
        session: &mut MeasurementAcquisitionSession,
        store: &mut S,
        catalog: &MeasurementAcquisitionCatalog,
        workspace: Option<&agentir_core::Workspace>,
        executor: &mut E,
        fault: Option<MeasurementAcquisitionRecoveryFaultBoundary>,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<RecoveryStatus>
    where
        S: MeasurementAcquisitionStore,
        E: MeasurementAcquisitionExecutor,
    {
        self.verify(session, store, catalog, limits)?;
        if !matches!(
            self.status,
            RecoveryStatus::Prepared | RecoveryStatus::RetryAuthorized
        ) {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "prepared slot execution requires prepared or retry-authorized status",
            ));
        }
        let mut staged_journal = self.clone();
        let mut staged_session = session.clone();
        let mut staged_store = store.clone();
        let prepared = staged_journal.current_prepared_slot()?.clone();
        if matches!(
            fault,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark)
        ) {
            staged_journal.status = RecoveryStatus::IndeterminateAfterCrash;
            staged_journal.work.execute_commands = add(staged_journal.work.execute_commands, 1)?;
            staged_journal.work.prevented_automatic_reruns =
                add(staged_journal.work.prevented_automatic_reruns, 1)?;
            staged_journal.push_trace(
                "crash_after_prepare_before_benchmark",
                Some(prepared.attempt_id),
                None,
                limits,
            )?;
            staged_journal.refresh_hash()?;
            *self = staged_journal;
            return Ok(self.status);
        }

        let slot = staged_session.recovery_pending_slot()?.clone();
        let benchmark = executor.benchmark(
            workspace,
            catalog,
            &staged_session.plan,
            &staged_session.preflight,
            &slot,
        );
        staged_journal.work.execute_commands = add(staged_journal.work.execute_commands, 1)?;
        staged_journal.work.benchmark_invocations =
            add(staged_journal.work.benchmark_invocations, 1)?;
        let (record, device_calls) = match benchmark {
            Ok(value) => value,
            Err(_) => {
                staged_journal.status = RecoveryStatus::IndeterminateAfterCrash;
                staged_journal.work.prevented_automatic_reruns =
                    add(staged_journal.work.prevented_automatic_reruns, 1)?;
                staged_journal.push_trace(
                    "benchmark_returned_without_publication",
                    Some(prepared.attempt_id),
                    None,
                    limits,
                )?;
                staged_journal.refresh_hash()?;
                *self = staged_journal;
                return Ok(self.status);
            }
        };
        validate_record(&staged_session, &slot, &record)?;
        staged_journal.work.device_calls = add(staged_journal.work.device_calls, device_calls)?;
        staged_journal
            .attempt_device_calls
            .insert(prepared.attempt_id.clone(), device_calls);
        if matches!(
            fault,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterBenchmarkBeforePublication)
        ) {
            staged_journal.status = RecoveryStatus::IndeterminateAfterCrash;
            staged_journal.work.prevented_automatic_reruns =
                add(staged_journal.work.prevented_automatic_reruns, 1)?;
            staged_journal.push_trace(
                "crash_after_benchmark_before_publication",
                Some(prepared.attempt_id),
                None,
                limits,
            )?;
            staged_journal.refresh_hash()?;
            *self = staged_journal;
            return Ok(self.status);
        }

        let (measurement_id, measurement_hash) = staged_store.publish(record)?;
        staged_journal.work.publications = add(staged_journal.work.publications, 1)?;
        if matches!(
            fault,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint)
        ) {
            staged_journal.status = RecoveryStatus::IndeterminateAfterCrash;
            staged_journal.work.prevented_automatic_reruns =
                add(staged_journal.work.prevented_automatic_reruns, 1)?;
            staged_journal.push_trace(
                "crash_after_publication_before_evaluation_checkpoint",
                Some(prepared.attempt_id),
                None,
                limits,
            )?;
            staged_journal.refresh_hash()?;
            *store = staged_store;
            *self = staged_journal;
            return Ok(self.status);
        }

        staged_session.attach_recovered_measurement(
            measurement_id,
            measurement_hash,
            device_calls,
        )?;
        staged_journal.status = if staged_session.status == MeasurementAcquisitionStatus::Complete {
            RecoveryStatus::Complete
        } else {
            RecoveryStatus::Reconciled
        };
        staged_journal.push_trace(
            if matches!(
                fault,
                Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterCheckpoint)
            ) {
                "crash_after_complete_evaluation_checkpoint"
            } else {
                "execute_published_and_checkpointed"
            },
            Some(prepared.attempt_id),
            Some(ReconciliationOutcome::Reconciled),
            limits,
        )?;
        staged_journal.refresh_hash()?;
        *store = staged_store;
        *session = staged_session;
        *self = staged_journal;
        Ok(self.status)
    }

    /// Reconciles production publications without benchmark/device calls.
    pub fn reconcile<S: MeasurementAcquisitionStore>(
        &mut self,
        session: &mut MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        current: &MeasurementAcquisitionRecoveryAnchors,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<MeasurementAcquisitionReconciliationResult> {
        self.verify(session, store, catalog, limits)?;
        if matches!(
            self.status,
            RecoveryStatus::Complete | RecoveryStatus::Reconciled
        ) {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionUnequalRecords,
                "completed acquisition slot cannot be reconciled again",
            ));
        }
        if self.status == RecoveryStatus::Abandoned {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "abandoned recovery journal cannot be reconciled",
            ));
        }
        let mut staged_journal = self.clone();
        let mut staged_session = session.clone();
        let prepared = staged_journal.current_prepared_slot()?.clone();
        let result_ordinal = add(count(staged_journal.reconciliation_results.len())?, 1)?;
        let mut local_work = MeasurementAcquisitionRecoveryWorkCounters {
            reconciliation_attempts: 1,
            ..MeasurementAcquisitionRecoveryWorkCounters::default()
        };

        let anchor_outcome = if current.workspace_id != prepared.workspace_id
            || current.root_anchor_hash != prepared.root_anchor_hash
            || catalog.workspace_id != prepared.workspace_id
            || catalog.root_anchor_hash != prepared.root_anchor_hash
        {
            Some(ReconciliationOutcome::WorkspaceChanged)
        } else if current.device_fingerprint_hash != prepared.device_fingerprint_hash {
            Some(ReconciliationOutcome::DeviceChanged)
        } else if current.compiler_build_hash != prepared.compiler_build_hash {
            Some(ReconciliationOutcome::BuildChanged)
        } else if current.runtime_version != prepared.runtime_version {
            Some(ReconciliationOutcome::RuntimeChanged)
        } else {
            None
        };

        let current_records = store.records();
        let baseline_by_id = prepared
            .publication_snapshot
            .records
            .iter()
            .map(|anchor| {
                (
                    anchor.measurement_id.clone(),
                    anchor.measurement_hash.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if baseline_by_id.len() != prepared.publication_snapshot.records.len() {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "prepared publication snapshot contains duplicate measurement IDs",
            ));
        }
        let current_by_id = current_records
            .iter()
            .map(|(id, record)| (id.clone(), record))
            .collect::<BTreeMap<_, _>>();
        for (id, expected_hash) in &baseline_by_id {
            let record = current_by_id.get(id).ok_or_else(|| {
                recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "a baseline publication disappeared before reconciliation",
                )
            })?;
            let calculated = measurement_hash(record)
                .map_err(|error| {
                    recovery_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        error.to_string(),
                    )
                })?
                .to_string();
            if &calculated != expected_hash || record.measurement_hash.as_str() != expected_hash {
                return Err(recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "a baseline publication changed before reconciliation",
                ));
            }
        }
        let new_records = current_records
            .into_iter()
            .filter(|(id, _)| !baseline_by_id.contains_key(id))
            .collect::<Vec<_>>();
        limit(
            count(new_records.len())?,
            limits.reconciliation_candidates,
            "reconciliation_candidates",
        )?;
        local_work.reconciliation_candidates = count(new_records.len())?;
        let mut compatible = Vec::new();
        let mut incompatible = false;
        for (id, record) in new_records {
            let calculated = measurement_hash(&record)
                .map_err(|error| {
                    recovery_error(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        error.to_string(),
                    )
                })?
                .to_string();
            if calculated != record.measurement_hash.to_string() {
                return Err(recovery_error(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "post-prepare production measurement is corrupt",
                ));
            }
            if compatible_record(&prepared, &record) {
                compatible.push((id, calculated));
            } else {
                incompatible = true;
            }
        }
        compatible.sort_by(|left, right| left.0.cmp(&right.0));

        let observed = anchor_outcome.unwrap_or_else(|| {
            if incompatible {
                ReconciliationOutcome::IncompatiblePublicationObserved
            } else {
                match compatible.len() {
                    0 => ReconciliationOutcome::NoPublicationObserved,
                    1 => ReconciliationOutcome::ExactlyOneCompatiblePublication,
                    _ => ReconciliationOutcome::MultipleCompatiblePublications,
                }
            }
        });
        let mut outcome = observed;
        let mut accepted_id = None;
        let mut accepted_hash = None;
        if observed == ReconciliationOutcome::ExactlyOneCompatiblePublication {
            let (id, hash) = compatible[0].clone();
            let device_calls = staged_journal
                .attempt_device_calls
                .get(&prepared.attempt_id)
                .copied()
                .unwrap_or_else(|| expected_device_calls(&prepared));
            staged_session.attach_recovered_measurement(id.clone(), hash.clone(), device_calls)?;
            staged_journal.status =
                if staged_session.status == MeasurementAcquisitionStatus::Complete {
                    RecoveryStatus::Complete
                } else {
                    RecoveryStatus::Reconciled
                };
            local_work.reconciled_publications = 1;
            outcome = ReconciliationOutcome::Reconciled;
            accepted_id = Some(id);
            accepted_hash = Some(hash);
        } else {
            staged_journal.status = match observed {
                ReconciliationOutcome::NoPublicationObserved => {
                    RecoveryStatus::NoPublicationObserved
                }
                ReconciliationOutcome::MultipleCompatiblePublications => RecoveryStatus::Ambiguous,
                _ => RecoveryStatus::Blocked,
            };
        }

        let mut result = MeasurementAcquisitionReconciliationResult {
            version: 1,
            reconciliation_id: format!(
                "{}-reconciliation-{}",
                staged_journal.recovery_journal_id, result_ordinal
            ),
            measurement_acquisition_prepared_slot_hash: prepared
                .measurement_acquisition_prepared_slot_hash
                .clone(),
            attempt_id: prepared.attempt_id.clone(),
            observed_outcome: observed,
            outcome,
            compatible_measurement_ids: compatible.iter().map(|(id, _)| id.clone()).collect(),
            compatible_measurement_hashes: compatible
                .iter()
                .map(|(_, hash)| hash.clone())
                .collect(),
            accepted_measurement_id: accepted_id,
            accepted_measurement_hash: accepted_hash,
            work: local_work.clone(),
            measurement_acquisition_reconciliation_hash: String::new(),
        };
        result.measurement_acquisition_reconciliation_hash =
            measurement_acquisition_reconciliation_hash(&result)?;
        staged_journal.reconciliation_results.push(result.clone());
        staged_journal.work.reconciliation_attempts =
            add(staged_journal.work.reconciliation_attempts, 1)?;
        staged_journal.work.reconciliation_candidates = add(
            staged_journal.work.reconciliation_candidates,
            local_work.reconciliation_candidates,
        )?;
        staged_journal.work.reconciled_publications = add(
            staged_journal.work.reconciled_publications,
            local_work.reconciled_publications,
        )?;
        staged_journal.work.prevented_automatic_reruns =
            add(staged_journal.work.prevented_automatic_reruns, 1)?;
        staged_journal.push_trace(
            "publication_reconciliation_completed",
            Some(prepared.attempt_id),
            Some(outcome),
            limits,
        )?;
        staged_journal.refresh_hash()?;
        *session = staged_session;
        *self = staged_journal;
        Ok(result)
    }

    /// Authorizes exactly one new attempt after a zero-publication result.
    pub fn authorize_retry<S: MeasurementAcquisitionStore>(
        &mut self,
        session: &MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<&MeasurementAcquisitionPreparedSlot> {
        self.verify(session, store, catalog, limits)?;
        if self.status != RecoveryStatus::NoPublicationObserved
            || self
                .reconciliation_results
                .last()
                .is_none_or(|result| result.outcome != ReconciliationOutcome::NoPublicationObserved)
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "retry requires a latest zero-publication reconciliation result",
            ));
        }
        limit(
            add(count(self.retry_authorizations.len())?, 1)?,
            limits.retry_attempts,
            "retry_attempts",
        )?;
        limit(
            add(count(self.prepared_slots.len())?, 1)?,
            limits.prepared_slots,
            "prepared_slots",
        )?;
        let mut staged = self.clone();
        let prior = staged.current_prepared_slot()?.clone();
        let slot = session.recovery_pending_slot()?;
        let snapshot = publication_snapshot(store, limits)?;
        let ordinal = add(count(staged.prepared_slots.len())?, 1)?;
        let prepared = prepared_from_slot(
            session,
            slot,
            snapshot,
            ordinal,
            MeasurementAcquisitionPreparationStatus::RetryAuthorized,
        )?;
        let reconciliation_hash = staged
            .reconciliation_results
            .last()
            .expect("validated latest reconciliation")
            .measurement_acquisition_reconciliation_hash
            .clone();
        staged
            .retry_authorizations
            .push(MeasurementAcquisitionRetryAuthorization {
                prior_attempt_id: prior.attempt_id,
                authorized_attempt_id: prepared.attempt_id.clone(),
                reconciliation_hash,
            });
        staged.work.retry_authorizations = add(staged.work.retry_authorizations, 1)?;
        staged.work.prepared_slots = add(staged.work.prepared_slots, 1)?;
        staged.work.snapshot_records = add(
            staged.work.snapshot_records,
            count(prepared.publication_snapshot.records.len())?,
        )?;
        staged.status = RecoveryStatus::RetryAuthorized;
        staged.prepared_slots.push(prepared.clone());
        staged.push_trace(
            "retry_explicitly_authorized",
            Some(prepared.attempt_id),
            Some(ReconciliationOutcome::RetryAuthorized),
            limits,
        )?;
        staged.refresh_hash()?;
        *self = staged;
        self.current_prepared_slot()
    }

    /// Explicitly abandons an unresolved prepared slot without hardware work.
    pub fn abandon(
        &mut self,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<RecoveryStatus> {
        if matches!(
            self.status,
            RecoveryStatus::Complete | RecoveryStatus::Reconciled | RecoveryStatus::Abandoned
        ) {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash,
                "completed, reconciled, or abandoned recovery cannot be abandoned again",
            ));
        }
        let mut staged = self.clone();
        let attempt = staged.current_prepared_slot()?.attempt_id.clone();
        staged.status = RecoveryStatus::Abandoned;
        staged.work.abandonments = add(staged.work.abandonments, 1)?;
        staged.push_trace(
            "recovery_explicitly_abandoned",
            Some(attempt),
            Some(ReconciliationOutcome::Abandoned),
            limits,
        )?;
        staged.refresh_hash()?;
        *self = staged;
        Ok(self.status)
    }

    /// Encodes a durable recovery checkpoint without hardware work.
    pub fn checkpoint(
        &mut self,
        session: &MeasurementAcquisitionSession,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<MeasurementAcquisitionRecoveryCheckpoint> {
        let mut staged = self.clone();
        staged.work.checkpoints = add(staged.work.checkpoints, 1)?;
        staged.refresh_hash()?;
        let mut checkpoint = MeasurementAcquisitionRecoveryCheckpoint {
            version: 1,
            journal: Box::new(staged.clone()),
            session: Box::new(session.clone()),
            measurement_acquisition_recovery_journal_hash: staged
                .measurement_acquisition_recovery_journal_hash
                .clone(),
            measurement_acquisition_recovery_checkpoint_hash: String::new(),
        };
        checkpoint.measurement_acquisition_recovery_checkpoint_hash =
            measurement_acquisition_recovery_checkpoint_hash(&checkpoint)?;
        let bytes = serde_json::to_vec(&checkpoint).map_err(|error| {
            recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                error.to_string(),
            )
        })?;
        limit(
            count(bytes.len())?,
            limits.checkpoint_bytes,
            "recovery_checkpoint_bytes",
        )?;
        *self = staged;
        Ok(checkpoint)
    }

    /// Restores one exact recovery checkpoint without executor/device access.
    pub fn restore_checkpoint<S: MeasurementAcquisitionStore>(
        checkpoint: &MeasurementAcquisitionRecoveryCheckpoint,
        session: &MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<Self> {
        if checkpoint.version != 1
            || checkpoint.measurement_acquisition_recovery_checkpoint_hash
                != measurement_acquisition_recovery_checkpoint_hash(checkpoint)?
            || checkpoint.measurement_acquisition_recovery_journal_hash
                != checkpoint
                    .journal
                    .measurement_acquisition_recovery_journal_hash
            || checkpoint.session.as_ref() != session
        {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                "recovery checkpoint hash is corrupt",
            ));
        }
        checkpoint.journal.verify(session, store, catalog, limits)?;
        Ok((*checkpoint.journal).clone())
    }

    /// Replays the exact recovery lifecycle with zero benchmark/device calls.
    pub fn replay<S: MeasurementAcquisitionStore>(
        &self,
        session: &MeasurementAcquisitionSession,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<MeasurementAcquisitionRecoveryResult> {
        self.verify(session, store, catalog, limits)?;
        let replay_work = add(
            add(count(self.trace.len())?, count(self.prepared_slots.len())?)?,
            count(self.reconciliation_results.len())?,
        )?;
        limit(replay_work, limits.replay_work, "recovery_replay_work")?;
        if self.work.replay_hardware_calls != 0 {
            return Err(recovery_error(
                EvaluationErrorCode::EvaluationAcquisitionReplayHardwareWork,
                "recovery replay attempted hardware work",
            ));
        }
        let mut projected = self.result();
        projected.work.replays = add(projected.work.replays, 1)?;
        projected.work.replay_hardware_calls = 0;
        Ok(projected)
    }

    /// Returns the exact current recovery result projection.
    #[must_use]
    pub fn result(&self) -> MeasurementAcquisitionRecoveryResult {
        let latest = self.reconciliation_results.last();
        MeasurementAcquisitionRecoveryResult {
            recovery_journal_id: self.recovery_journal_id.clone(),
            status: self.status,
            slot_index: self.slot_index,
            latest_outcome: latest.map(|result| result.outcome),
            accepted_measurement_id: latest
                .and_then(|result| result.accepted_measurement_id.clone()),
            accepted_measurement_hash: latest
                .and_then(|result| result.accepted_measurement_hash.clone()),
            measurement_acquisition_recovery_journal_hash: self
                .measurement_acquisition_recovery_journal_hash
                .clone(),
            work: self.work.clone(),
        }
    }

    fn push_trace(
        &mut self,
        kind: &str,
        attempt_id: Option<String>,
        outcome: Option<ReconciliationOutcome>,
        limits: &MeasurementAcquisitionRecoveryLimits,
    ) -> EvaluationResult<()> {
        let sequence = count(self.trace.len())?;
        limit(
            add(sequence, 1)?,
            limits.recovery_trace_events,
            "recovery_trace_events",
        )?;
        self.trace.push(MeasurementAcquisitionRecoveryTraceEvent {
            sequence,
            kind: kind.to_owned(),
            attempt_id,
            slot_index: self.slot_index,
            outcome,
        });
        Ok(())
    }

    fn refresh_hash(&mut self) -> EvaluationResult<()> {
        self.measurement_acquisition_recovery_journal_hash =
            measurement_acquisition_recovery_journal_hash(self)?;
        Ok(())
    }
}

fn compatible_record(
    prepared: &MeasurementAcquisitionPreparedSlot,
    record: &HardwareMeasurementRecord,
) -> bool {
    let expected_validation = match prepared.validation_policy {
        MeasurementValidationPolicy::HardwareExecutedV1 => "offline_validated_and_device_executed",
        MeasurementValidationPolicy::SyntheticFixtureV1 => {
            "synthetic_test_data_not_performance_evidence"
        }
    };
    record.artifact_hash.as_str() == prepared.artifact_hash
        && record.target_hash.as_str() == prepared.target_hash
        && record.compiler_build_hash.as_str() == prepared.compiler_build_hash
        && record.device_fingerprint_hash.as_str() == prepared.device_fingerprint_hash
        && record.runtime_version == prepared.runtime_version
        && record.config == prepared.benchmark_config
        && record.validation_status == expected_validation
}

fn expected_device_calls(prepared: &MeasurementAcquisitionPreparedSlot) -> u64 {
    if prepared.device_fingerprint_hash.is_empty() {
        0
    } else {
        u64::from(prepared.benchmark_config.warmups)
            .saturating_add(u64::from(prepared.benchmark_config.iterations))
    }
}
