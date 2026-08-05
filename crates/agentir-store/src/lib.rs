//! Versioned, checksummed, atomic file persistence for AgentIR workspaces.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{RevisionId, WorkspaceId},
    persistence::{ReplayReport, WorkspaceSnapshot},
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
pub const ARCHIVE_FORMAT_VERSION: u32 = 1;

/// Stable archive format discriminator.
pub const ARCHIVE_KIND: &str = "agentir.workspace";

/// Maximum archive size accepted by the Stage 1 local store.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveBody {
    format: String,
    format_version: u32,
    compiler_version: String,
    snapshot: WorkspaceSnapshot,
}

/// Self-checking on-disk workspace envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceArchive {
    /// Stable format discriminator.
    pub format: String,
    /// On-disk format version.
    pub format_version: u32,
    /// AgentIR crate version that wrote the archive.
    pub compiler_version: String,
    /// Complete compiler-core snapshot.
    pub snapshot: WorkspaceSnapshot,
    /// SHA-256 of the deterministic archive body.
    pub archive_hash: String,
}

/// Metadata returned after saving or loading an archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveMetadata {
    /// On-disk format version.
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

/// Restored workspace plus integrity and replay evidence.
#[derive(Debug)]
pub struct LoadedWorkspace {
    /// Verified live workspace ready for new transactions.
    pub workspace: Workspace,
    /// Archive metadata.
    pub metadata: ArchiveMetadata,
    /// Deterministic replay report.
    pub replay: ReplayReport,
}

fn io_error(operation: &str, path: &Path, error: &std::io::Error) -> AgentError {
    AgentError::new(
        ErrorCode::PersistenceIo,
        format!("{operation} failed for `{}`: {error}", path.display()),
    )
    .with_detail("path", path.display().to_string())
    .with_detail("io_kind", format!("{:?}", error.kind()))
}

fn body(archive: &WorkspaceArchive) -> ArchiveBody {
    ArchiveBody {
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

fn body_hash(body: &ArchiveBody) -> AgentResult<String> {
    let bytes = serde_json::to_vec(body).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("workspace archive serialization failed: {error}"),
        )
    })?;
    Ok(sha256(&bytes))
}

fn metadata(archive: &WorkspaceArchive, bytes: usize) -> ArchiveMetadata {
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

fn decode_archive(path: &Path) -> AgentResult<(WorkspaceArchive, usize)> {
    let file_metadata =
        fs::metadata(path).map_err(|error| io_error("metadata read", path, &error))?;
    if file_metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            format!(
                "archive size {} exceeds Stage 1 limit {}",
                file_metadata.len(),
                MAX_ARCHIVE_BYTES
            ),
        ));
    }
    let mut file = fs::File::open(path).map_err(|error| io_error("archive open", path, &error))?;
    let mut bytes = Vec::with_capacity(usize::try_from(file_metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error("archive read", path, &error))?;
    let archive: WorkspaceArchive = serde_json::from_slice(&bytes).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("archive JSON is invalid: {error}"),
        )
        .with_detail("path", path.display().to_string())
    })?;
    if archive.format != ARCHIVE_KIND || archive.format_version != ARCHIVE_FORMAT_VERSION {
        return Err(AgentError::new(
            ErrorCode::PersistenceFormat,
            format!(
                "unsupported archive `{}` version {}; expected `{}` version {}",
                archive.format, archive.format_version, ARCHIVE_KIND, ARCHIVE_FORMAT_VERSION
            ),
        ));
    }
    let actual_hash = body_hash(&body(&archive))?;
    if actual_hash != archive.archive_hash {
        return Err(AgentError::new(
            ErrorCode::PersistenceIntegrity,
            "workspace archive checksum does not match its body",
        )
        .with_detail("expected_hash", archive.archive_hash.clone())
        .with_detail("actual_hash", actual_hash));
    }
    Ok((archive, bytes.len()))
}

/// Writes a checksummed workspace archive using a same-directory temporary file and rename.
pub fn save_workspace(
    path: impl AsRef<Path>,
    workspace: &Workspace,
) -> AgentResult<ArchiveMetadata> {
    let path = path.as_ref();
    let body = ArchiveBody {
        format: ARCHIVE_KIND.to_owned(),
        format_version: ARCHIVE_FORMAT_VERSION,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot: workspace.snapshot(),
    };
    let archive = WorkspaceArchive {
        format: body.format.clone(),
        format_version: body.format_version,
        compiler_version: body.compiler_version.clone(),
        snapshot: body.snapshot.clone(),
        archive_hash: body_hash(&body)?,
    };
    let mut encoded = serde_json::to_vec(&archive).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("workspace archive encoding failed: {error}"),
        )
    })?;
    encoded.push(b'\n');
    let temporary = temp_path(path)?;
    let write_result = (|| -> AgentResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| io_error("temporary archive create", &temporary, &error))?;
        file.write_all(&encoded)
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
    write_result?;
    Ok(metadata(&archive, encoded.len()))
}

/// Loads an archive, verifies its checksum, and deterministically replays its event log.
pub fn load_workspace(path: impl AsRef<Path>) -> AgentResult<LoadedWorkspace> {
    let path = path.as_ref();
    let (archive, bytes) = decode_archive(path)?;
    let archive_metadata = metadata(&archive, bytes);
    let (workspace, replay) = Workspace::from_snapshot(archive.snapshot)?;
    Ok(LoadedWorkspace {
        workspace,
        metadata: archive_metadata,
        replay,
    })
}

/// Verifies an archive and returns replay evidence without retaining the workspace.
pub fn verify_archive(path: impl AsRef<Path>) -> AgentResult<(ArchiveMetadata, ReplayReport)> {
    let loaded = load_workspace(path)?;
    Ok((loaded.metadata, loaded.replay))
}
