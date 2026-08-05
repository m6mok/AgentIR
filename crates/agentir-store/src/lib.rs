//! Versioned, checksummed, atomic file persistence for AgentIR workspaces.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{RevisionId, WorkspaceId},
    persistence::{
        LegacyWorkspaceSnapshotV1, LegacyWorkspaceSnapshotV2, ReplayReport,
        VersionedWorkspaceEvent, WORKSPACE_SNAPSHOT_VERSION, WorkspaceSnapshot,
        migrate_snapshot_v1, migrate_snapshot_v2,
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
pub const ARCHIVE_FORMAT_VERSION: u32 = 3;

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

/// Stable name used to report an explicit v3-to-v3 no-op.
pub const MIGRATION_V3_NOOP: &str = "workspace_archive_v3_noop";

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
        target_version: ARCHIVE_FORMAT_VERSION,
        name: MIGRATION_V2_TO_V3,
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

/// Current workspace archive type retained as a convenient API alias.
pub type WorkspaceArchive = WorkspaceArchiveV3;

/// Current self-checking workspace archive with semantics-versioned events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchiveV3 {
    /// Stable format discriminator.
    pub format: String,
    /// Current on-disk format version, always three.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Current compiler-core snapshot schema.
    pub snapshot: WorkspaceSnapshot,
    /// SHA-256 of the deterministic v3 archive body.
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
        LEGACY_ARCHIVE_FORMAT_VERSION | LEGACY_ARCHIVE_FORMAT_V2 | ARCHIVE_FORMAT_VERSION
    ) {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            format!(
                "unsupported archive version {}; supported versions are {}, {}, and {}",
                header.format_version,
                LEGACY_ARCHIVE_FORMAT_VERSION,
                LEGACY_ARCHIVE_FORMAT_V2,
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
        ARCHIVE_FORMAT_VERSION => {
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
        _ => unreachable!("validated archive version"),
    }
}

fn current_archive(snapshot: WorkspaceSnapshot) -> AgentResult<WorkspaceArchiveV3> {
    let body = ArchiveBodyV3 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: ARCHIVE_FORMAT_VERSION,
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

fn encode_archive(archive: &WorkspaceArchiveV3) -> AgentResult<Vec<u8>> {
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
    let (workspace, _replay) = Workspace::from_legacy_migrated_snapshot(migrated_v3)?;
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

/// Purely verifies and migrates an immutable archive v2 value to current v3.
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
    current_archive(migrate_snapshot_v2(archive.snapshot)?)
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
            let migrated = migrate_snapshot_v2(migrate_snapshot_v1(archive.snapshot)?)?;
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
            let migrated = migrate_snapshot_v2(archive.snapshot)?;
            let report = MigrationReport {
                source_archive_version: LEGACY_ARCHIVE_FORMAT_V2,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![MIGRATION_V2_TO_V3.to_owned()],
                workspace: migrated.workspace.clone(),
                head: migrated.head.clone(),
                revisions: migrated.revisions.len(),
                old_archive_hash: archive.archive_hash,
                new_archive_hash: None,
            };
            Ok((migrated, metadata, report, false))
        }
        DecodedArchive::V3(archive) => {
            if archive.snapshot.schema_version != WORKSPACE_SNAPSHOT_VERSION {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "archive v3 snapshot schema {} is unsupported; expected {}",
                        archive.snapshot.schema_version, WORKSPACE_SNAPSHOT_VERSION
                    ),
                ));
            }
            validate_snapshot_counts(
                archive.snapshot.revisions.len(),
                archive.snapshot.events.len(),
                versioned_event_actions(&archive.snapshot.events),
            )?;
            let metadata = metadata_v3(&archive, bytes);
            let report = MigrationReport {
                source_archive_version: ARCHIVE_FORMAT_VERSION,
                target_archive_version: ARCHIVE_FORMAT_VERSION,
                applied_steps: vec![MIGRATION_V3_NOOP.to_owned()],
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

/// Encodes a workspace as current archive format version 3 without filesystem I/O.
pub fn encode_workspace_archive(workspace: &Workspace) -> AgentResult<Vec<u8>> {
    encode_archive(&current_archive(workspace.snapshot())?)
}

/// Writes a checksummed v3 workspace archive using a same-directory temporary file and rename.
pub fn save_workspace(
    path: impl AsRef<Path>,
    workspace: &Workspace,
) -> AgentResult<ArchiveMetadata> {
    let path = path.as_ref();
    let archive = current_archive(workspace.snapshot())?;
    let encoded = encode_archive(&archive)?;
    write_atomic(path, &encoded)?;
    Ok(metadata_v3(&archive, encoded.len()))
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

/// Fully verifies a source archive and atomically writes its current v3 representation.
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
