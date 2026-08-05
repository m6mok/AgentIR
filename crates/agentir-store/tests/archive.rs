use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    persistence::{LegacyWorkspaceSnapshotV1, WorkspaceSnapshot},
    semantic::SpecHash,
};
use agentir_store::{
    ARCHIVE_FORMAT_VERSION, LEGACY_ARCHIVE_FORMAT_VERSION, MIGRATION_V1_TO_V2, MIGRATION_V2_NOOP,
    WorkspaceArchiveV1, WorkspaceArchiveV2, load_workspace, load_workspace_bytes, migrate_archive,
    migrate_archive_v1_to_v2, save_workspace, verify_archive,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MINIMAL_V1: &[u8] = include_bytes!("fixtures/minimal-v1.json");
const SAXPY_V1: &[u8] = include_bytes!("fixtures/saxpy-v1.json");
const MINIMAL_V2: &[u8] = include_bytes!("fixtures/minimal-v2.json");
const CORRUPTED_V1: &[u8] = include_bytes!("fixtures/corrupted-v1.json");
const FUTURE_V3: &[u8] = include_bytes!("fixtures/future-v3.json");

static PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestArchive(PathBuf);

impl TestArchive {
    fn new(label: &str) -> Self {
        let sequence = PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "agentir-{label}-{}-{sequence}.json",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, bytes: &[u8]) {
        fs::write(&self.0, bytes).expect("fixture write");
    }
}

impl Drop for TestArchive {
    fn drop(&mut self) {
        let _ignored = fs::remove_file(&self.0);
    }
}

fn simple_workspace() -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new("persisted")).expect("workspace");
    workspace
        .apply(&Transaction {
            workspace: WorkspaceId::new("persisted"),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().expect("type"),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$x".to_owned(),
                },
            ],
            client_transaction_id: Some("initial".to_owned()),
            allow_branch: false,
        })
        .expect("transaction");
    workspace
}

fn hash_body(body: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(body).expect("body JSON");
    let digest = Sha256::digest(bytes);
    digest.iter().fold(
        String::with_capacity(digest.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        },
    )
}

#[derive(Serialize)]
struct BodyV1<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV1,
}

#[derive(Serialize)]
struct BodyV2<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a WorkspaceSnapshot,
}

#[test]
fn archive_round_trip_writes_v2_replays_and_resumes_ids() {
    let archive = TestArchive::new("round-trip");
    let workspace = simple_workspace();
    let expected = workspace.snapshot();
    let saved = save_workspace(archive.path(), &workspace).expect("saves");
    assert_eq!(saved.format_version, ARCHIVE_FORMAT_VERSION);
    assert_eq!(saved.revisions, 2);
    assert_eq!(saved.events, 1);

    let mut loaded = load_workspace(archive.path()).expect("loads");
    assert_eq!(loaded.workspace.snapshot(), expected);
    assert_eq!(loaded.replay.revisions_verified, 2);
    assert_eq!(loaded.replay.content_hashes_verified, 2);
    assert_eq!(loaded.metadata.archive_hash, saved.archive_hash);
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V2_NOOP]);

    let resumed = loaded
        .workspace
        .apply(&Transaction {
            workspace: WorkspaceId::new("persisted"),
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::CreateConstant {
                bind: "$one".to_owned(),
                ty: "f32".parse().expect("type"),
                value: json!(1.0),
            }],
            client_transaction_id: Some("resumed".to_owned()),
            allow_branch: false,
        })
        .expect("resumed transaction");
    assert_eq!(resumed.revision, RevisionId::new("r2"));
    assert_eq!(resumed.bindings["$one"], "v2");
}

#[test]
fn golden_v1_migrates_purely_and_preserves_content_hashes() {
    let legacy: WorkspaceArchiveV1 = serde_json::from_slice(MINIMAL_V1).expect("legacy fixture");
    let expected_hashes = legacy
        .snapshot
        .revisions
        .iter()
        .map(|(id, revision)| (id.clone(), revision.content_hash.clone()))
        .collect::<BTreeMap<_, _>>();
    let migrated = migrate_archive_v1_to_v2(legacy).expect("pure migration");
    assert_eq!(migrated.format_version, ARCHIVE_FORMAT_VERSION);
    assert_eq!(migrated.snapshot.schema_version, 2);
    for (id, hash) in &expected_hashes {
        assert_eq!(migrated.snapshot.revisions[id].content_hash, *hash);
    }
    let frozen = &migrated.snapshot.revisions[&RevisionId::new("r2")];
    assert!(frozen.spec_hash.is_some());
    assert_eq!(frozen.semantic_canonical_version, Some(1));

    let loaded = load_workspace_bytes(MINIMAL_V1).expect("migrated load");
    assert_eq!(
        loaded.metadata.format_version,
        LEGACY_ARCHIVE_FORMAT_VERSION
    );
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V1_TO_V2]);
    assert_eq!(loaded.replay.spec_hashes_verified, 1);
}

#[test]
fn golden_saxpy_v1_migrates_and_evaluates() {
    let loaded = load_workspace_bytes(SAXPY_V1).expect("SAXPY migration");
    let program = &loaded
        .workspace
        .revision(&RevisionId::new("r2"))
        .expect("frozen revision")
        .program;
    let result = agentir_eval::evaluate(
        program,
        &BTreeMap::from([
            ("a".to_owned(), json!(2.0)),
            ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
            ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
        ]),
    )
    .expect("evaluation");
    assert_eq!(result.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
}

#[test]
fn saving_a_loaded_v1_workspace_writes_only_v2() {
    let destination = TestArchive::new("save-migrated");
    let loaded = load_workspace_bytes(MINIMAL_V1).expect("v1 load");
    let saved = save_workspace(destination.path(), &loaded.workspace).expect("v2 save");
    assert_eq!(saved.format_version, ARCHIVE_FORMAT_VERSION);
    let document: Value =
        serde_json::from_slice(&fs::read(destination.path()).expect("read")).expect("JSON");
    assert_eq!(document["format_version"], ARCHIVE_FORMAT_VERSION);
    assert_eq!(document["snapshot"]["schema_version"], 2);
}

#[test]
fn golden_v2_load_is_an_explicit_noop() {
    let loaded = load_workspace_bytes(MINIMAL_V2).expect("v2 load");
    assert_eq!(loaded.metadata.format_version, ARCHIVE_FORMAT_VERSION);
    assert_eq!(loaded.migration.source_archive_version, 2);
    assert_eq!(loaded.migration.target_archive_version, 2);
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V2_NOOP]);
}

#[test]
fn corrupted_and_future_fixtures_are_rejected_at_the_versioned_boundary() {
    let checksum = load_workspace_bytes(CORRUPTED_V1).expect_err("checksum fails");
    assert_eq!(checksum.code, ErrorCode::PersistenceIntegrity);
    let future = load_workspace_bytes(FUTURE_V3).expect_err("future version fails");
    assert_eq!(future.code, ErrorCode::PersistenceFormat);
    let zero = load_workspace_bytes(br#"{"format":"agentir.workspace","format_version":0}"#)
        .expect_err("version zero fails");
    assert_eq!(zero.code, ErrorCode::PersistenceFormat);
}

#[test]
fn revision_hash_corruption_is_rejected_after_envelope_verification() {
    let mut archive: WorkspaceArchiveV1 =
        serde_json::from_slice(MINIMAL_V1).expect("legacy fixture");
    archive
        .snapshot
        .revisions
        .get_mut(&RevisionId::new("r2"))
        .expect("revision")
        .content_hash = "tampered".to_owned();
    archive.archive_hash = hash_body(&BodyV1 {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    });
    let bytes = serde_json::to_vec(&archive).expect("archive JSON");
    let error = load_workspace_bytes(&bytes).expect_err("revision hash fails");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
}

#[test]
fn cached_spec_hash_is_checked_during_v2_load() {
    let mut archive: WorkspaceArchiveV2 = serde_json::from_slice(MINIMAL_V2).expect("v2 fixture");
    archive
        .snapshot
        .revisions
        .get_mut(&RevisionId::new("r2"))
        .expect("revision")
        .spec_hash = Some(SpecHash::new("tampered"));
    archive.archive_hash = hash_body(&BodyV2 {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    });
    let bytes = serde_json::to_vec(&archive).expect("archive JSON");
    let error = load_workspace_bytes(&bytes).expect_err("spec hash fails");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
}

#[test]
fn failed_migration_leaves_no_partial_destination() {
    let source = TestArchive::new("bad-source");
    let destination = TestArchive::new("partial-destination");
    source.write(CORRUPTED_V1);
    assert!(!destination.path().exists());
    let error = migrate_archive(source.path(), destination.path(), false)
        .expect_err("corrupt migration fails");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
    assert!(!destination.path().exists());
}

#[test]
fn existing_destination_requires_overwrite_and_in_place_is_explicit() {
    let source = TestArchive::new("migration-source");
    let destination = TestArchive::new("migration-destination");
    source.write(MINIMAL_V1);
    destination.write(b"preserve me");
    let error = migrate_archive(source.path(), destination.path(), false)
        .expect_err("overwrite is required");
    assert_eq!(error.code, ErrorCode::PersistenceIo);
    assert_eq!(
        fs::read(destination.path()).expect("destination"),
        b"preserve me"
    );

    let report = migrate_archive(source.path(), destination.path(), true).expect("overwrite");
    assert!(report.new_archive_hash.is_some());
    assert_eq!(
        load_workspace(destination.path())
            .expect("migrated destination")
            .metadata
            .format_version,
        2
    );

    let in_place = migrate_archive(source.path(), source.path(), true).expect("in-place migration");
    assert_eq!(in_place.source_archive_version, 1);
    assert_eq!(
        load_workspace(source.path())
            .expect("in-place result")
            .metadata
            .format_version,
        2
    );
}

#[test]
fn archive_round_trip_property_harness_is_deterministic() {
    for seed in 0_u64..24 {
        let mut workspace =
            Workspace::new(WorkspaceId::new(format!("roundtrip-{seed}"))).expect("workspace");
        let count = usize::try_from(seed % 7 + 1).expect("small count");
        let mut actions = Vec::new();
        for index in 0..count {
            actions.push(Action::CreateConstant {
                bind: format!("$constant{index}"),
                ty: "i32".parse().expect("type"),
                value: json!(seed + u64::try_from(index).expect("index")),
            });
        }
        actions.push(Action::SetOutput {
            name: "out".to_owned(),
            value: format!("$constant{}", count - 1),
        });
        let built = workspace
            .apply(&Transaction {
                workspace: workspace.id().clone(),
                base_revision: RevisionId::new("r0"),
                actions,
                client_transaction_id: Some(format!("seed-{seed}")),
                allow_branch: false,
            })
            .expect("build");
        workspace
            .apply(&Transaction {
                workspace: workspace.id().clone(),
                base_revision: built.revision,
                actions: vec![Action::FreezeSpec],
                client_transaction_id: None,
                allow_branch: false,
            })
            .expect("freeze");
        let first = agentir_store::encode_workspace_archive(&workspace).expect("encode");
        let loaded = load_workspace_bytes(&first).expect("load");
        let second = agentir_store::encode_workspace_archive(&loaded.workspace).expect("reencode");
        assert_eq!(first, second, "seed {seed}");
    }
}

#[test]
fn archive_checksum_tampering_is_rejected() {
    let archive = TestArchive::new("tamper");
    save_workspace(archive.path(), &simple_workspace()).expect("saves");
    let mut document: Value =
        serde_json::from_slice(&fs::read(archive.path()).expect("reads")).expect("JSON");
    document["archive_hash"] = Value::String("tampered".to_owned());
    archive.write(&serde_json::to_vec(&document).expect("encodes"));
    let error = verify_archive(archive.path()).expect_err("tampering fails");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
}
