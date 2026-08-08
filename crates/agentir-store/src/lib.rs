//! Versioned, checksummed, atomic file persistence for AgentIR workspaces.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{RevisionId, WorkspaceId},
    persistence::{
        LegacyWorkspaceSnapshotV1, LegacyWorkspaceSnapshotV2, LegacyWorkspaceSnapshotV3,
        LegacyWorkspaceSnapshotV4, LegacyWorkspaceSnapshotV5, LegacyWorkspaceSnapshotV6,
        LegacyWorkspaceSnapshotV7, ReplayReport, VersionedWorkspaceEvent,
        WORKSPACE_SNAPSHOT_VERSION, WorkspaceSnapshot, migrate_snapshot_v1, migrate_snapshot_v2,
        migrate_snapshot_v3, migrate_snapshot_v4, migrate_snapshot_v5, migrate_snapshot_v6,
        migrate_snapshot_v7,
    },
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    workspace::Workspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Current on-disk archive format version.
pub const ARCHIVE_FORMAT_VERSION: u32 = 8;

/// Immutable Stage 3 archive format version retained as a legacy input.
pub const LEGACY_ARCHIVE_FORMAT_V7: u32 = 7;

/// Immutable Stage 2C archive format version retained as a legacy input.
pub const LEGACY_ARCHIVE_FORMAT_V6: u32 = 6;

/// Immutable Stage 2B archive format version retained as a legacy input.
pub const LEGACY_ARCHIVE_FORMAT_V5: u32 = 5;

/// Immutable Stage 2A archive format version.
pub const LEGACY_ARCHIVE_FORMAT_V4: u32 = 4;

/// Immutable Stage 1.2 archive format version.
pub const LEGACY_ARCHIVE_FORMAT_V3: u32 = 3;

/// Immutable Stage 1.1 archive format version.
pub const LEGACY_ARCHIVE_FORMAT_V2: u32 = 2;

/// Immutable legacy on-disk archive format version.
pub const LEGACY_ARCHIVE_FORMAT_VERSION: u32 = 1;

/// Stable archive format discriminator.
pub const ARCHIVE_KIND: &str = "agentir.workspace";

/// Maximum archive size accepted by the local store.
pub const MAX_ARCHIVE_BYTES: u64 = ResourceLimits::hard_safety_caps().archive_bytes;

/// Stable name of the only Stage 1.1 migration step.
pub const MIGRATION_V1_TO_V2: &str = "workspace_archive_v1_to_v2";

/// Stable name of the Stage 1.2 migration that tags legacy events.
pub const MIGRATION_V2_TO_V3: &str = "workspace_archive_v2_to_v3";

/// Stable name of the Stage 2A migration that adds an empty CandidateForest.
pub const MIGRATION_V3_TO_V4: &str = "workspace_archive_v3_to_v4";

/// Stable name of the Stage 2B migration that preserves candidate hash v1.
pub const MIGRATION_V4_TO_V5: &str = "workspace_archive_v4_to_v5";

/// Stable name of the Stage 2C migration that adds an empty equality store.
pub const MIGRATION_V5_TO_V6: &str = "workspace_archive_v5_to_v6";

/// Stable name of the Stage 3 migration that adds an empty MemoryIR store.
pub const MIGRATION_V6_TO_V7: &str = "workspace_archive_v6_to_v7";

/// Stable name of the Stage 4 migration that adds target and schedule stores.
pub const MIGRATION_V7_TO_V8: &str = "workspace_archive_v7_to_v8";

/// Stable name used by historical reports for an explicit v3-to-v3 no-op.
pub const MIGRATION_V3_NOOP: &str = "workspace_archive_v3_noop";

/// Stable name used to report an explicit v4-to-v4 no-op.
pub const MIGRATION_V4_NOOP: &str = "workspace_archive_v4_noop";

/// Stable name used to report an explicit v5-to-v5 no-op.
pub const MIGRATION_V5_NOOP: &str = "workspace_archive_v5_noop";

/// Stable name used to report an explicit v6-to-v6 no-op.
pub const MIGRATION_V6_NOOP: &str = "workspace_archive_v6_noop";

/// Stable name used to report an explicit v7-to-v7 no-op.
pub const MIGRATION_V7_NOOP: &str = "workspace_archive_v7_noop";

/// Stable name used to report an explicit v8-to-v8 no-op.
pub const MIGRATION_V8_NOOP: &str = "workspace_archive_v8_noop";

/// Retained Stage 1.1 report name; v2 source loads now use `MIGRATION_V2_TO_V3`.
pub const MIGRATION_V2_NOOP: &str = "workspace_archive_v2_noop";

/// One registered pure archive migration edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationStep {
    /// Source archive version accepted by the step.
    pub source_version: u32,
    /// Target archive version produced by the step.
    pub target_version: u32,
    /// Stable report name.
    pub name: &'static str,
}

/// Ordered registry of supported archive migration edges.
pub const ARCHIVE_MIGRATIONS: &[MigrationStep] = &[
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_VERSION,
        target_version: LEGACY_ARCHIVE_FORMAT_V2,
        name: MIGRATION_V1_TO_V2,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V2,
        target_version: LEGACY_ARCHIVE_FORMAT_V3,
        name: MIGRATION_V2_TO_V3,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V3,
        target_version: LEGACY_ARCHIVE_FORMAT_V4,
        name: MIGRATION_V3_TO_V4,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V4,
        target_version: LEGACY_ARCHIVE_FORMAT_V5,
        name: MIGRATION_V4_TO_V5,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V5,
        target_version: LEGACY_ARCHIVE_FORMAT_V6,
        name: MIGRATION_V5_TO_V6,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V6,
        target_version: LEGACY_ARCHIVE_FORMAT_V7,
        name: MIGRATION_V6_TO_V7,
    },
    MigrationStep {
        source_version: LEGACY_ARCHIVE_FORMAT_V7,
        target_version: ARCHIVE_FORMAT_VERSION,
        name: MIGRATION_V7_TO_V8,
    },
];

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV1 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV2 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV3 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV4 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV4,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV5 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV5,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV6 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV6,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV7 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: LegacyWorkspaceSnapshotV7,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBodyV8 {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: WorkspaceSnapshot,
}

#[derive(Debug, Deserialize)]
struct ArchiveHeader {
    format: String,
    format_version: u32,
}

/// Immutable self-checking archive format published by commit `97c821a`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV1 {
    /// Stable format discriminator.
    pub format: String,
    /// Legacy on-disk format version, always one.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Legacy compiler-core snapshot schema.
    pub snapshot: LegacyWorkspaceSnapshotV1,
    /// SHA-256 of the deterministic v1 archive body.
    pub archive_hash: String,
}

/// Immutable self-checking workspace archive format published by Stage 1.1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV2 {
    /// Stable format discriminator.
    pub format: String,
    /// Legacy on-disk format version, always two.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Legacy compiler-core snapshot schema.
    pub snapshot: LegacyWorkspaceSnapshotV2,
    /// SHA-256 of the deterministic v2 archive body.
    pub archive_hash: String,
}

/// Immutable Stage 1.2 workspace archive with semantics-versioned SpecIR events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV3 {
    /// Stable format discriminator.
    pub format: String,
    /// Legacy on-disk format version, always three.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Immutable Stage 1.2 compiler-core snapshot schema.
    pub snapshot: LegacyWorkspaceSnapshotV3,
    /// SHA-256 of the deterministic v3 archive body.
    pub archive_hash: String,
}

/// Immutable Stage 2A workspace archive containing CandidateForest v1 and EvidenceIR.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV4 {
    /// Stable format discriminator.
    pub format: String,
    /// Current on-disk format version, always four.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Immutable Stage 2A compiler-core snapshot schema.
    pub snapshot: LegacyWorkspaceSnapshotV4,
    /// SHA-256 of the deterministic v4 archive body.
    pub archive_hash: String,
}

/// Current workspace archive type retained as a convenient API alias.
pub type WorkspaceArchive = WorkspaceArchiveV8;

/// Immutable self-checking v5 workspace archive with Stage 2B candidate state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV5 {
    /// Stable format discriminator.
    pub format: String,
    /// Current on-disk format version, always five.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Immutable compiler-core snapshot schema v5.
    pub snapshot: LegacyWorkspaceSnapshotV5,
    /// SHA-256 of the deterministic v5 archive body.
    pub archive_hash: String,
}

/// Immutable self-checking v6 workspace archive with Stage 2C equality state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV6 {
    /// Stable format discriminator.
    pub format: String,
    /// Legacy on-disk format version, always six.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Immutable compiler-core snapshot schema v6.
    pub snapshot: LegacyWorkspaceSnapshotV6,
    /// SHA-256 of the deterministic v6 archive body.
    pub archive_hash: String,
}

/// Current self-checking v7 workspace archive with Stage 3 MemoryIR state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV7 {
    /// Stable format discriminator.
    pub format: String,
    /// Current on-disk format version, always seven.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Current compiler-core snapshot schema v7.
    pub snapshot: LegacyWorkspaceSnapshotV7,
    /// SHA-256 of the deterministic v7 archive body.
    pub archive_hash: String,
}

/// Current self-checking v8 workspace archive with TargetManifest and ScheduleIR state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV8 {
    /// Stable format discriminator.
    pub format: String,
    /// Current on-disk format version, always eight.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Current compiler-core snapshot schema v8.
    pub snapshot: WorkspaceSnapshot,
    /// SHA-256 of the deterministic v8 archive body.
    pub archive_hash: String,
}

/// Metadata returned after saving or loading an archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveMetadata {
    /// Source or written on-disk format version.
    pub format_version: u32,
    /// Workspace stored by the archive.
    pub workspace: WorkspaceId,
    /// Archived head revision.
    pub head: RevisionId,
    /// Number of immutable revisions.
    pub revisions: usize,
    /// Number of replayable state-changing events.
    pub events: usize,
    /// Archive body hash.
    pub archive_hash: String,
    /// Encoded archive size in bytes.
    pub bytes: usize,
}

/// Deterministic evidence describing an archive migration pipeline execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// Version discovered and verified at the source boundary.
    pub source_archive_version: u32,
    /// Current target version after the migration pipeline.
    pub target_archive_version: u32,
    /// Stable migration step names in application order.
    pub applied_steps: Vec<String>,
    /// Migrated workspace identity.
    pub workspace: WorkspaceId,
    /// Migrated head revision.
    pub head: RevisionId,
    /// Number of migrated revisions.
    pub revisions: usize,
    /// Integrity hash of the verified source archive.
    pub old_archive_hash: String,
    /// Integrity hash of a newly written destination archive, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_archive_hash: Option<String>,
}

/// Restored workspace plus integrity, migration, and replay evidence.
#[derive(Debug)]
pub struct LoadedWorkspace {
    /// Verified live workspace ready for new transactions.
    pub workspace: Workspace,
    /// Metadata for the source archive as it existed on disk.
    pub metadata: ArchiveMetadata,
    /// Explicit migration or no-op report.
    pub migration: MigrationReport,
    /// Deterministic replay report.
    pub replay: ReplayReport,
}

enum DecodedArchive {
    V1(WorkspaceArchiveV1),
    V2(WorkspaceArchiveV2),
    V3(WorkspaceArchiveV3),
    V4(WorkspaceArchiveV4),
    V5(WorkspaceArchiveV5),
    V6(WorkspaceArchiveV6),
    V7(WorkspaceArchiveV7),
    V8(WorkspaceArchiveV8),
}

fn io_error(operation: &str, path: &Path, error: &std::io::Error) -> AgentError {
    AgentError::new(
        ErrorCode::PersistenceIo,
        format!("{operation} failed for `{}`: {error}", path.display()),
    )
    .with_detail("path", path.display().to_string())
    .with_detail("io_kind", format!("{:?}", error.kind()))
}

fn v1_body(archive: &WorkspaceArchiveV1) -> ArchiveBodyV1 {
    ArchiveBodyV1 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v2_body(archive: &WorkspaceArchiveV2) -> ArchiveBodyV2 {
    ArchiveBodyV2 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v3_body(archive: &WorkspaceArchiveV3) -> ArchiveBodyV3 {
    ArchiveBodyV3 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v4_body(archive: &WorkspaceArchiveV4) -> ArchiveBodyV4 {
    ArchiveBodyV4 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v5_body(archive: &WorkspaceArchiveV5) -> ArchiveBodyV5 {
    ArchiveBodyV5 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v6_body(archive: &WorkspaceArchiveV6) -> ArchiveBodyV6 {
    ArchiveBodyV6 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v7_body(archive: &WorkspaceArchiveV7) -> ArchiveBodyV7 {
    ArchiveBodyV7 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn v8_body(archive: &WorkspaceArchiveV8) -> ArchiveBodyV8 {
    ArchiveBodyV8 {
        format: archive.format.clone(),
        format_version: archive.format_version,
        compiler_version: archive.compiler_version.clone(),
        snapshot: archive.snapshot.clone(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn serialized_hash<T: Serialize>(body: &T) -> AgentResult<String> {
    let bytes = serde_json::to_vec(body).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("workspace archive serialization failed: {error}"),
        )
    })?;
    Ok(sha256(&bytes))
}

fn metadata_v1(archive: &WorkspaceArchiveV1, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive.snapshot.events.len(),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v2(archive: &WorkspaceArchiveV2, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive.snapshot.events.len(),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v3(archive: &WorkspaceArchiveV3, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive.snapshot.events.len(),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v4(archive: &WorkspaceArchiveV4, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive
            .snapshot
            .events
            .len()
            .saturating_add(archive.snapshot.candidate_event_count()),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v5(archive: &WorkspaceArchiveV5, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive
            .snapshot
            .events
            .len()
            .saturating_add(archive.snapshot.candidate_forest.events.len()),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v6(archive: &WorkspaceArchiveV6, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive
            .snapshot
            .events
            .len()
            .saturating_add(archive.snapshot.candidate_forest.events.len())
            .saturating_add(archive.snapshot.equality_store.events.len()),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v7(archive: &WorkspaceArchiveV7, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive
            .snapshot
            .events
            .len()
            .saturating_add(archive.snapshot.candidate_forest.events.len())
            .saturating_add(archive.snapshot.equality_store.events.len())
            .saturating_add(archive.snapshot.memory_store.events.len()),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn metadata_v8(archive: &WorkspaceArchiveV8, bytes: usize) -> ArchiveMetadata {
    ArchiveMetadata {
        format_version: archive.format_version,
        workspace: archive.snapshot.workspace.clone(),
        head: archive.snapshot.head.clone(),
        revisions: archive.snapshot.revisions.len(),
        events: archive
            .snapshot
            .events
            .len()
            .saturating_add(archive.snapshot.candidate_forest.events.len())
            .saturating_add(archive.snapshot.equality_store.events.len())
            .saturating_add(archive.snapshot.memory_store.events.len())
            .saturating_add(archive.snapshot.target_store.events.len())
            .saturating_add(archive.snapshot.schedule_store.events.len()),
        archive_hash: archive.archive_hash.clone(),
        bytes,
    }
}

fn validate_snapshot_counts(revisions: usize, events: usize, actions: u64) -> AgentResult<()> {
    let limits = ResourceLimits::hard_safety_caps();
    validate_snapshot_counts_with_limits(revisions, events, actions, &limits)
}

fn validate_snapshot_counts_with_limits(
    revisions: usize,
    events: usize,
    actions: u64,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::RevisionsPerArchive,
        u64::try_from(revisions).unwrap_or(u64::MAX),
        "archive snapshot preflight",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::EventsPerArchive,
        u64::try_from(events).unwrap_or(u64::MAX),
        "archive snapshot preflight",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ActionsReplayedPerArchive,
        actions,
        "archive replay preflight",
    )
}

fn validate_candidate_snapshot_counts(
    forest: &agentir_core::candidate::CandidateForest,
) -> AgentResult<()> {
    let limits = ResourceLimits::hard_safety_caps();
    let revisions = forest.candidates.values().fold(0_u64, |total, candidate| {
        total.saturating_add(u64::try_from(candidate.revisions.len()).unwrap_or(u64::MAX))
    });
    for (resource, actual) in [
        (
            ResourceKind::CandidatesPerWorkspace,
            u64::try_from(forest.candidates.len()).unwrap_or(u64::MAX),
        ),
        (ResourceKind::CandidateRevisionsPerWorkspace, revisions),
        (
            ResourceKind::CandidateEventsPerArchive,
            u64::try_from(forest.events.len()).unwrap_or(u64::MAX),
        ),
        (
            ResourceKind::EvidenceRecords,
            u64::try_from(forest.evidence.len()).unwrap_or(u64::MAX),
        ),
    ] {
        BudgetCheck::against(&limits, resource, actual, "candidate archive preflight")?;
    }
    let evidence_bytes = serde_json::to_vec(&forest.evidence).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("candidate evidence preflight encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        &limits,
        ResourceKind::EvidenceBytes,
        u64::try_from(evidence_bytes.len()).unwrap_or(u64::MAX),
        "candidate archive preflight",
    )
}

fn validate_equality_snapshot_counts(
    store: &agentir_core::equality::EqualityStore,
) -> AgentResult<()> {
    let limits = ResourceLimits::hard_safety_caps();
    let revisions = store.spaces.values().fold(0_u64, |total, space| {
        total.saturating_add(u64::try_from(space.revisions.len()).unwrap_or(u64::MAX))
    });
    for (resource, actual) in [
        (
            ResourceKind::EqualitySpacesPerWorkspace,
            u64::try_from(store.spaces.len()).unwrap_or(u64::MAX),
        ),
        (ResourceKind::EqualityRevisionsPerWorkspace, revisions),
        (
            ResourceKind::EqualityEvents,
            u64::try_from(store.events.len()).unwrap_or(u64::MAX),
        ),
    ] {
        BudgetCheck::against(&limits, resource, actual, "equality archive preflight")?;
    }
    let bytes = serde_json::to_vec(store).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("equality archive preflight encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        &limits,
        ResourceKind::EqualityArchiveBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "equality archive preflight",
    )
}

fn validate_legacy_candidate_snapshot_counts(
    snapshot: &LegacyWorkspaceSnapshotV4,
) -> AgentResult<()> {
    let limits = ResourceLimits::hard_safety_caps();
    let (candidates, revisions, evidence) = snapshot.candidate_counts();
    for (resource, actual) in [
        (
            ResourceKind::CandidatesPerWorkspace,
            u64::try_from(candidates).unwrap_or(u64::MAX),
        ),
        (ResourceKind::CandidateRevisionsPerWorkspace, revisions),
        (
            ResourceKind::CandidateEventsPerArchive,
            u64::try_from(snapshot.candidate_event_count()).unwrap_or(u64::MAX),
        ),
        (
            ResourceKind::EvidenceRecords,
            u64::try_from(evidence).unwrap_or(u64::MAX),
        ),
    ] {
        BudgetCheck::against(
            &limits,
            resource,
            actual,
            "legacy candidate archive preflight",
        )?;
    }
    BudgetCheck::against(
        &limits,
        ResourceKind::EvidenceBytes,
        snapshot.evidence_encoded_bytes()?,
        "legacy candidate archive preflight",
    )
}

fn legacy_event_actions(events: &[agentir_core::persistence::WorkspaceEvent]) -> u64 {
    events.iter().fold(0_u64, |total, event| {
        total.saturating_add(match event {
            agentir_core::persistence::WorkspaceEvent::TransactionApplied {
                transaction, ..
            } => u64::try_from(transaction.actions.len()).unwrap_or(u64::MAX),
            agentir_core::persistence::WorkspaceEvent::RevisionForked { .. } => 0,
        })
    })
}

fn versioned_event_actions(events: &[VersionedWorkspaceEvent]) -> u64 {
    events.iter().fold(0_u64, |total, event| {
        total.saturating_add(legacy_event_actions(std::slice::from_ref(&event.event)))
    })
}

fn temp_path(path: &Path) -> AgentResult<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::PersistenceIo,
                format!("archive path `{}` has no valid file name", path.display()),
            )
        })?;
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    )))
}

fn read_limited(path: &Path) -> AgentResult<Vec<u8>> {
    let file = fs::File::open(path).map_err(|error| io_error("archive open", path, &error))?;
    let mut bytes = Vec::new();
    file.take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("archive read", path, &error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ARCHIVE_BYTES {
        return BudgetCheck::ensure(
            ResourceKind::ArchiveBytes,
            MAX_ARCHIVE_BYTES,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            "bounded archive read",
        )
        .map(|()| bytes);
    }
    Ok(bytes)
}

fn validate_header(header: &ArchiveHeader) -> AgentResult<()> {
    if header.format != ARCHIVE_KIND {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            format!(
                "unsupported archive format `{}`; expected `{ARCHIVE_KIND}`",
                header.format
            ),
        ));
    }
    if !matches!(
        header.format_version,
        LEGACY_ARCHIVE_FORMAT_VERSION
            | LEGACY_ARCHIVE_FORMAT_V2
            | LEGACY_ARCHIVE_FORMAT_V3
            | LEGACY_ARCHIVE_FORMAT_V4
            | LEGACY_ARCHIVE_FORMAT_V5
            | LEGACY_ARCHIVE_FORMAT_V6
            | LEGACY_ARCHIVE_FORMAT_V7
            | ARCHIVE_FORMAT_VERSION
    ) {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            format!(
                "unsupported archive version {}; supported versions are {}, {}, {}, {}, {}, {}, {}, and {}",
                header.format_version,
                LEGACY_ARCHIVE_FORMAT_VERSION,
                LEGACY_ARCHIVE_FORMAT_V2,
                LEGACY_ARCHIVE_FORMAT_V3,
                LEGACY_ARCHIVE_FORMAT_V4,
                LEGACY_ARCHIVE_FORMAT_V5,
                LEGACY_ARCHIVE_FORMAT_V6,
                LEGACY_ARCHIVE_FORMAT_V7,
                ARCHIVE_FORMAT_VERSION
            ),
        ));
    }
    Ok(())
}

fn decode_archive_bytes(bytes: &[u8]) -> AgentResult<DecodedArchive> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ARCHIVE_BYTES {
        BudgetCheck::ensure(
            ResourceKind::ArchiveBytes,
            MAX_ARCHIVE_BYTES,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            "archive decode",
        )?;
    }
    let header: ArchiveHeader = serde_json::from_slice(bytes).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("archive JSON header is invalid: {error}"),
        )
    })?;
    validate_header(&header)?;
    match header.format_version {
        LEGACY_ARCHIVE_FORMAT_VERSION => {
            let archive: WorkspaceArchiveV1 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v1 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v1_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v1 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V1(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V2 => {
            let archive: WorkspaceArchiveV2 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v2 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v2_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v2 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V2(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V3 => {
            let archive: WorkspaceArchiveV3 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v3 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v3_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v3 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V3(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V4 => {
            let archive: WorkspaceArchiveV4 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v4 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v4_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v4 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V4(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V5 => {
            let archive: WorkspaceArchiveV5 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v5 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v5_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v5 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V5(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V6 => {
            let archive: WorkspaceArchiveV6 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v6 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v6_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v6 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V6(archive))
        }
        LEGACY_ARCHIVE_FORMAT_V7 => {
            let archive: WorkspaceArchiveV7 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v7 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v7_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v7 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V7(archive))
        }
        ARCHIVE_FORMAT_VERSION => {
            let archive: WorkspaceArchiveV8 = serde_json::from_slice(bytes).map_err(|error| {
                AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!("archive v8 JSON is invalid: {error}"),
                )
            })?;
            let actual_hash = serialized_hash(&v8_body(&archive))?;
            if actual_hash != archive.archive_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    "workspace archive v8 checksum does not match its body",
                )
                .with_detail("expected_hash", archive.archive_hash.clone())
                .with_detail("actual_hash", actual_hash));
            }
            Ok(DecodedArchive::V8(archive))
        }
        _ => unreachable!("validated archive version"),
    }
}

fn current_archive(snapshot: WorkspaceSnapshot) -> AgentResult<WorkspaceArchiveV8> {
    let body = ArchiveBodyV8 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: ARCHIVE_FORMAT_VERSION,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
    };
    Ok(WorkspaceArchiveV8 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

fn encode_archive(archive: &WorkspaceArchiveV8) -> AgentResult<Vec<u8>> {
    let mut encoded = serde_json::to_vec(archive).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("workspace archive encoding failed: {error}"),
        )
    })?;
    encoded.push(b'\n');
    BudgetCheck::against(
        &ResourceLimits::hard_safety_caps(),
        ResourceKind::ArchiveBytes,
        u64::try_from(encoded.len()).unwrap_or(u64::MAX),
        "workspace archive encoding",
    )?;
    Ok(encoded)
}

fn write_atomic(path: &Path, encoded: &[u8]) -> AgentResult<()> {
    let temporary = temp_path(path)?;
    let write_result = (|| -> AgentResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| io_error("temporary archive create", &temporary, &error))?;
        file.write_all(encoded)
            .map_err(|error| io_error("temporary archive write", &temporary, &error))?;
        file.sync_all()
            .map_err(|error| io_error("temporary archive sync", &temporary, &error))?;
        fs::rename(&temporary, path)
            .map_err(|error| io_error("atomic archive rename", path, &error))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ignored = fs::remove_file(&temporary);
    }
    write_result
}

/// Purely verifies and migrates an archive v1 value to archive and snapshot version 2.
pub fn migrate_archive_v1_to_v2(archive: WorkspaceArchiveV1) -> AgentResult<WorkspaceArchiveV2> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_VERSION {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v1 migration received a non-v1 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v1_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v1 checksum does not match its body",
        ));
    }
    let migrated_v2 = migrate_snapshot_v1(archive.snapshot)?;
    let migrated_v3 = migrate_snapshot_v2(migrated_v2)?;
    let migrated_v4 = migrate_snapshot_v3(migrated_v3)?;
    let migrated_v5 = migrate_snapshot_v4(migrated_v4)?;
    let migrated_v6 = migrate_snapshot_v5(migrated_v5)?;
    let migrated_v7 = migrate_snapshot_v6(migrated_v6)?;
    let migrated_v8 = migrate_snapshot_v7(migrated_v7)?;
    let (workspace, _replay) = Workspace::from_legacy_migrated_snapshot(migrated_v8)?;
    let snapshot = workspace.snapshot();
    let snapshot = LegacyWorkspaceSnapshotV2 {
        schema_version: LEGACY_ARCHIVE_FORMAT_V2,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot
            .events
            .into_iter()
            .map(|event| event.event)
            .collect(),
    };
    let body = ArchiveBodyV2 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V2,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
    };
    Ok(WorkspaceArchiveV2 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates an immutable archive v2 value to archive v3.
pub fn migrate_archive_v2_to_v3(archive: WorkspaceArchiveV2) -> AgentResult<WorkspaceArchiveV3> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V2 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v2 migration received a non-v2 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v2_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v2 checksum does not match its body",
        ));
    }
    let snapshot = migrate_snapshot_v2(archive.snapshot)?;
    let body = ArchiveBodyV3 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V3,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
    };
    Ok(WorkspaceArchiveV3 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates an immutable archive v3 value to current v4.
pub fn migrate_archive_v3_to_v4(archive: WorkspaceArchiveV3) -> AgentResult<WorkspaceArchiveV4> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V3 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v3 migration received a non-v3 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v3_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v3 checksum does not match its body",
        ));
    }
    let body = ArchiveBodyV4 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V4,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot: migrate_snapshot_v3(archive.snapshot)?,
    };
    Ok(WorkspaceArchiveV4 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates immutable archive v4 to immutable archive v5.
pub fn migrate_archive_v4_to_v5(archive: WorkspaceArchiveV4) -> AgentResult<WorkspaceArchiveV5> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V4 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v4 migration received a non-v4 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v4_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v4 checksum does not match its body",
        ));
    }
    let body = ArchiveBodyV5 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V5,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot: migrate_snapshot_v4(archive.snapshot)?,
    };
    Ok(WorkspaceArchiveV5 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates immutable archive v5 to immutable archive v6.
pub fn migrate_archive_v5_to_v6(archive: WorkspaceArchiveV5) -> AgentResult<WorkspaceArchiveV6> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V5 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v5 migration received a non-v5 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v5_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v5 checksum does not match its body",
        ));
    }
    let body = ArchiveBodyV6 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V6,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot: migrate_snapshot_v5(archive.snapshot)?,
    };
    Ok(WorkspaceArchiveV6 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates immutable archive v6 to current archive v7.
pub fn migrate_archive_v6_to_v7(archive: WorkspaceArchiveV6) -> AgentResult<WorkspaceArchiveV7> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V6 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v6 migration received a non-v6 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v6_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v6 checksum does not match its body",
        ));
    }
    let snapshot = migrate_snapshot_v6(archive.snapshot)?;
    let body = ArchiveBodyV7 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V7,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
    };
    Ok(WorkspaceArchiveV7 {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: serialized_hash(&body)?,
    })
}

/// Purely verifies and migrates immutable archive v7 to current archive v8.
pub fn migrate_archive_v7_to_v8(archive: WorkspaceArchiveV7) -> AgentResult<WorkspaceArchiveV8> {
    if archive.format != ARCHIVE_KIND || archive.format_version != LEGACY_ARCHIVE_FORMAT_V7 {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            "v7 migration received a non-v7 workspace archive",
        ));
    }
    let actual_hash = serialized_hash(&v7_body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive v7 checksum does not match its body",
        ));
    }
    let snapshot = migrate_snapshot_v7(archive.snapshot)?;
    Workspace::from_snapshot(snapshot.clone())?;
    current_archive(snapshot)
}

fn prepare(
    decoded: DecodedArchive,
    bytes: usize,
) -> AgentResult<(WorkspaceSnapshot, ArchiveMetadata, MigrationReport, bool)> {
    match decoded {
        DecodedArchive::V1(archive) => {
            let metadata = metadata_v1(&archive, bytes);
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                legacy_event_actions(&archive.snapshot.events),
            )?;
            let migrated = migrate_snapshot_v6(migrate_snapshot_v5(migrate_snapshot_v4(
                migrate_snapshot_v3(migrate_snapshot_v2(migrate_snapshot_v1(archive.snapshot)?)?)?,
            )?)?)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_VERSION,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: ARCHIVE_MIGRATIONS
                    .iter()
                    .map(|step| step.name.to_owned())
                    .collect(),
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash: metadata.archive_hash.clone(),
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, true))
        }
        DecodedArchive::V2(archive) => {
            if archive.snapshot.schema_version != LEGACY_ARCHIVE_FORMAT_V2 {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v2 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version, LEGACY_ARCHIVE_FORMAT_V2
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                legacy_event_actions(&archive.snapshot.events),
            )?;
            let metadata = metadata_v2(&archive, bytes);
            let migrated = migrate_snapshot_v6(migrate_snapshot_v5(migrate_snapshot_v4(
                migrate_snapshot_v3(migrate_snapshot_v2(archive.snapshot)?)?,
            )?)?)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V2,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![
                    MIGRATION_V2_TO_V3.to_owned(),
                    MIGRATION_V3_TO_V4.to_owned(),
                    MIGRATION_V4_TO_V5.to_owned(),
                    MIGRATION_V5_TO_V6.to_owned(),
                    MIGRATION_V6_TO_V7.to_owned(),
                    MIGRATION_V7_TO_V8.to_owned(),
                ],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash: archive.archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V3(archive) => {
            if archive.snapshot.schema_version != LEGACY_ARCHIVE_FORMAT_V3 {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v3 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version, LEGACY_ARCHIVE_FORMAT_V3
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            let metadata = metadata_v3(&archive, bytes);
            let migrated = migrate_snapshot_v6(migrate_snapshot_v5(migrate_snapshot_v4(
                migrate_snapshot_v3(archive.snapshot)?,
            )?)?)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V3,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![
                    MIGRATION_V3_TO_V4.to_owned(),
                    MIGRATION_V4_TO_V5.to_owned(),
                    MIGRATION_V5_TO_V6.to_owned(),
                    MIGRATION_V6_TO_V7.to_owned(),
                    MIGRATION_V7_TO_V8.to_owned(),
                ],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash: archive.archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V4(archive) => {
            if archive.snapshot.schema_version != LEGACY_ARCHIVE_FORMAT_V4 {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v4 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version, LEGACY_ARCHIVE_FORMAT_V4
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            validate_legacy_candidate_snapshot_counts(&archive.snapshot)?;
            let metadata = metadata_v4(&archive, bytes);
            let migrated =
                migrate_snapshot_v6(migrate_snapshot_v5(migrate_snapshot_v4(archive.snapshot)?)?)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V4,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![
                    MIGRATION_V4_TO_V5.to_owned(),
                    MIGRATION_V5_TO_V6.to_owned(),
                    MIGRATION_V6_TO_V7.to_owned(),
                    MIGRATION_V7_TO_V8.to_owned(),
                ],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash: archive.archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V5(archive) => {
            if archive.snapshot.schema_version
                != agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION
            {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v5 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version,
                        agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            validate_candidate_snapshot_counts(&archive.snapshot.candidate_forest)?;
            let metadata = metadata_v5(&archive, bytes);
            let old_archive_hash = archive.archive_hash.clone();
            let migrated = migrate_snapshot_v6(migrate_snapshot_v5(archive.snapshot)?)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V5,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![
                    MIGRATION_V5_TO_V6.to_owned(),
                    MIGRATION_V6_TO_V7.to_owned(),
                    MIGRATION_V7_TO_V8.to_owned(),
                ],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V6(archive) => {
            if archive.snapshot.schema_version
                != agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION
            {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v6 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version,
                        agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            validate_candidate_snapshot_counts(&archive.snapshot.candidate_forest)?;
            validate_equality_snapshot_counts(&archive.snapshot.equality_store)?;
            let metadata = metadata_v6(&archive, bytes);
            let old_archive_hash = archive.archive_hash.clone();
            let migrated = migrate_snapshot_v6(archive.snapshot)?;
            let migrated = migrate_snapshot_v7(migrated)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V6,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![MIGRATION_V6_TO_V7.to_owned(), MIGRATION_V7_TO_V8.to_owned()],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V7(archive) => {
            if archive.snapshot.schema_version
                != agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION
            {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v7 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version,
                        agentir_core::persistence::LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            validate_candidate_snapshot_counts(&archive.snapshot.candidate_forest)?;
            validate_equality_snapshot_counts(&archive.snapshot.equality_store)?;
            let metadata = metadata_v7(&archive, bytes);
            let old_archive_hash = archive.archive_hash.clone();
            let migrated = migrate_snapshot_v7(archive.snapshot)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V7,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![MIGRATION_V7_TO_V8.to_owned()],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V8(archive) => {
            if archive.snapshot.schema_version != WORKSPACE_SNAPSHOT_VERSION {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v8 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version, WORKSPACE_SNAPSHOT_VERSION
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            validate_candidate_snapshot_counts(&archive.snapshot.candidate_forest)?;
            validate_equality_snapshot_counts(&archive.snapshot.equality_store)?;
            let metadata = metadata_v8(&archive, bytes);
            let report = MigrationReport {
                source_archive_version: ARCHIVE_FORMAT_VERSION,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![MIGRATION_V8_NOOP.to_owned()],
                workspace: archive.snapshot.workspace.clone(),
                head: archive.snapshot.head.clone(),
                revisions: archive.snapshot.revisions.len(),
                old_archive_hash: archive.archive_hash,
                new_archive_hash: None,
            };
            Ok((archive.snapshot, metadata, report, false))
        }
    }
}

/// Encodes a workspace as current archive format version 8 without filesystem I/O.
pub fn encode_workspace_archive(workspace: &Workspace) -> AgentResult<Vec<u8>> {
    encode_archive(&current_archive(workspace.snapshot())?)
}

/// Writes a checksummed v8 workspace archive using a same-directory temporary file and rename.
pub fn save_workspace(
    path: impl AsRef<Path>,
    workspace: &Workspace,
) -> AgentResult<ArchiveMetadata> {
    let path = path.as_ref();
    let archive = current_archive(workspace.snapshot())?;
    let encoded = encode_archive(&archive)?;
    write_atomic(path, &encoded)?;
    Ok(metadata_v8(&archive, encoded.len()))
}

/// Loads archive bytes, migrates if needed, and verifies deterministic event replay.
pub fn load_workspace_bytes(bytes: &[u8]) -> AgentResult<LoadedWorkspace> {
    let decoded = decode_archive_bytes(bytes)?;
    let (snapshot, metadata, migration, populate_legacy_cache) = prepare(decoded, bytes.len())?;
    let (workspace, replay) = if populate_legacy_cache {
        Workspace::from_legacy_migrated_snapshot(snapshot)?
    } else {
        Workspace::from_snapshot(snapshot)?
    };
    Ok(LoadedWorkspace {
        workspace,
        metadata,
        migration,
        replay,
    })
}

/// Loads an archive, verifies its source checksum, migrates, and replays its event log.
pub fn load_workspace(path: impl AsRef<Path>) -> AgentResult<LoadedWorkspace> {
    let path = path.as_ref();
    let bytes = read_limited(path)?;
    load_workspace_bytes(&bytes).map_err(|mut error| {
        error
            .details
            .entry("path".to_owned())
            .or_insert_with(|| path.display().to_string().into());
        error
    })
}

/// Verifies an archive and returns replay evidence without retaining the workspace.
pub fn verify_archive(path: impl AsRef<Path>) -> AgentResult<(ArchiveMetadata, ReplayReport)> {
    let loaded = load_workspace(path)?;
    Ok((loaded.metadata, loaded.replay))
}

/// Fully verifies a source archive and atomically writes its current v8 representation.
pub fn migrate_archive(
    source_path: impl AsRef<Path>,
    destination_path: impl AsRef<Path>,
    overwrite: bool,
) -> AgentResult<MigrationReport> {
    let source_path = source_path.as_ref();
    let destination_path = destination_path.as_ref();
    let loaded = load_workspace(source_path)?;
    if destination_path.exists() && !overwrite {
        return Err(AgentError::new(
            ErrorCode::PersistenceIo,
            format!(
                "destination archive `{}` already exists; set overwrite to true",
                destination_path.display()
            ),
        )
        .with_detail("path", destination_path.display().to_string())
        .with_detail("overwrite_required", true));
    }
    if source_path == destination_path && !overwrite {
        return Err(AgentError::new(
            ErrorCode::PersistenceIo,
            "migrating in place requires overwrite to be true",
        ));
    }
    let saved = save_workspace(destination_path, &loaded.workspace)?;
    let mut report = loaded.migration;
    report.new_archive_hash = Some(saved.archive_hash);
    Ok(report)
}

#[cfg(test)]
mod resource_tests {
    use super::validate_snapshot_counts_with_limits;
    use agentir_core::{diagnostics::ErrorCode, resources::ResourceLimits};

    #[test]
    fn replay_preflight_accepts_exact_counts_and_rejects_limit_plus_one() {
        let limits = ResourceLimits {
            revisions_per_archive: 2,
            events_per_archive: 3,
            actions_replayed_per_archive: 5,
            ..ResourceLimits::default()
        };
        validate_snapshot_counts_with_limits(2, 3, 5, &limits).expect("exact limits accepted");
        let error = validate_snapshot_counts_with_limits(2, 4, 5, &limits)
            .expect_err("event limit + 1 rejected before replay");
        assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    }
}
