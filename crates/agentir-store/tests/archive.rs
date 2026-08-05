use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{CandidateAction, CandidateTransaction, FOLD_SCALAR_CONSTANTS_RULE, RelationKind},
    ids::{CandidateId, CandidateRevisionId, ImplOperationId},
    persistence::{
        CORE_SEMANTICS_VERSION, LEGACY_CORE_SEMANTICS_VERSION, LegacyWorkspaceSnapshotV1,
        LegacyWorkspaceSnapshotV2, LegacyWorkspaceSnapshotV3, LegacyWorkspaceSnapshotV4,
        LegacyWorkspaceSnapshotV5, LegacyWorkspaceSnapshotV6, WorkspaceSnapshot,
    },
    semantic::SpecHash,
};
use agentir_store::{
    ARCHIVE_FORMAT_VERSION, LEGACY_ARCHIVE_FORMAT_V2, LEGACY_ARCHIVE_FORMAT_V3,
    LEGACY_ARCHIVE_FORMAT_V4, LEGACY_ARCHIVE_FORMAT_V5, LEGACY_ARCHIVE_FORMAT_VERSION,
    MIGRATION_V1_TO_V2, MIGRATION_V2_TO_V3, MIGRATION_V3_TO_V4, MIGRATION_V4_TO_V5,
    MIGRATION_V5_TO_V6, MIGRATION_V6_TO_V7, MIGRATION_V7_NOOP, WorkspaceArchiveV1,
    WorkspaceArchiveV2, WorkspaceArchiveV3, WorkspaceArchiveV4, WorkspaceArchiveV5,
    WorkspaceArchiveV6, WorkspaceArchiveV7, load_workspace, load_workspace_bytes, migrate_archive,
    migrate_archive_v1_to_v2, migrate_archive_v2_to_v3, migrate_archive_v6_to_v7, save_workspace,
    verify_archive,
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
const MINIMAL_V3: &[u8] = include_bytes!("fixtures/minimal-v3.json");
const SAXPY_V3: &[u8] = include_bytes!("fixtures/saxpy-v3.json");
const MIXED_V3: &[u8] = include_bytes!("fixtures/mixed-v3.json");
const CORRUPTED_SEMANTICS_V3: &[u8] = include_bytes!("fixtures/corrupted-semantics-v3.json");
const CORRUPTED_V1: &[u8] = include_bytes!("fixtures/corrupted-v1.json");
const FUTURE_V3: &[u8] = include_bytes!("fixtures/future-v3.json");
const FUTURE_V4: &[u8] = include_bytes!("fixtures/future-v4.json");
const MINIMAL_V4: &[u8] = include_bytes!("fixtures/minimal-v4.json");
const SAXPY_FROZEN_V4: &[u8] = include_bytes!("fixtures/saxpy-frozen-v4.json");
const SAXPY_IDENTITY_V4: &[u8] = include_bytes!("fixtures/saxpy-identity-v4.json");
const REWRITTEN_SEALED_V4: &[u8] = include_bytes!("fixtures/candidate-rewrite-sealed-v4.json");
const MIGRATED_CANDIDATE_V4: &[u8] = include_bytes!("fixtures/migrated-v3-candidate-v4.json");
const CORRUPTED_CANDIDATE_SEMANTICS_V4: &[u8] =
    include_bytes!("fixtures/corrupted-candidate-semantics-v4.json");
const CORRUPTED_IMPL_HASH_V4: &[u8] = include_bytes!("fixtures/corrupted-impl-hash-v4.json");
const CORRUPTED_CANDIDATE_HASH_V4: &[u8] =
    include_bytes!("fixtures/corrupted-candidate-hash-v4.json");
const CORRUPTED_SPEC_ANCHOR_V4: &[u8] = include_bytes!("fixtures/corrupted-spec-anchor-v4.json");
const CORRUPTED_EVIDENCE_CHAIN_V4: &[u8] =
    include_bytes!("fixtures/corrupted-evidence-chain-v4.json");
const MINIMAL_V5: &[u8] = include_bytes!("fixtures/minimal-v5.json");
const MIGRATED_V4_EXACT_V5: &[u8] = include_bytes!("fixtures/migrated-v4-exact-v5.json");
const SPECULATIVE_OPEN_V5: &[u8] = include_bytes!("fixtures/speculative-open-v5.json");
const RECOGNIZED_V5: &[u8] = include_bytes!("fixtures/recognized-known-rewrite-v5.json");
const GUARDED_V5: &[u8] = include_bytes!("fixtures/guarded-candidate-v5.json");
const SEALED_GUARDED_V5: &[u8] = include_bytes!("fixtures/sealed-guarded-v5.json");
const REFUTED_V5: &[u8] = include_bytes!("fixtures/refuted-candidate-v5.json");
const MIXED_CANDIDATE_V5: &[u8] = include_bytes!("fixtures/mixed-candidate-semantics-v5.json");
const CORRUPTED_PROPOSAL_HASH_V5: &[u8] =
    include_bytes!("fixtures/corrupted-proposal-hash-v5.json");
const CORRUPTED_FRONTIER_V5: &[u8] = include_bytes!("fixtures/corrupted-proof-frontier-v5.json");
const CORRUPTED_DEBT_V5: &[u8] = include_bytes!("fixtures/corrupted-debt-chain-v5.json");
const CORRUPTED_GUARD_V5: &[u8] = include_bytes!("fixtures/corrupted-guard-v5.json");
const CORRUPTED_FALLBACK_V5: &[u8] = include_bytes!("fixtures/corrupted-fallback-v5.json");
const CORRUPTED_CANDIDATE_HASH_V2_V5: &[u8] =
    include_bytes!("fixtures/corrupted-candidate-hash-v2-v5.json");
const CORRUPTED_CANDIDATE_SEMANTICS_V2_V5: &[u8] =
    include_bytes!("fixtures/corrupted-candidate-semantics-v2-v5.json");
const FUTURE_V6: &[u8] = include_bytes!("fixtures/future-v6.json");
const FUTURE_V7: &[u8] = include_bytes!("fixtures/future-v7.json");
const FUTURE_V5: &[u8] = include_bytes!("fixtures/future-v5.json");
const MINIMAL_V6: &[u8] = include_bytes!("fixtures/minimal-v6.json");
const EQUALITY_ROOT_V6: &[u8] = include_bytes!("fixtures/equality-root-v6.json");
const EQUALITY_PARTIAL_V6: &[u8] = include_bytes!("fixtures/equality-partially-expanded-v6.json");
const EQUALITY_SATURATED_V6: &[u8] = include_bytes!("fixtures/equality-saturated-v6.json");
const EQUALITY_MERGED_V6: &[u8] = include_bytes!("fixtures/equality-merged-paths-v6.json");
const EQUALITY_DISCHARGED_V6: &[u8] = include_bytes!("fixtures/equality-discharged-v6.json");
const EQUALITY_MATERIALIZED_V6: &[u8] = include_bytes!("fixtures/equality-materialized-v6.json");
const MIXED_CANDIDATE_V6: &[u8] = include_bytes!("fixtures/mixed-candidate-semantics-v6.json");
const CORRUPTED_EQUALITY_NODE_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-node-hash-v6.json");
const CORRUPTED_EQUALITY_EDGE_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-edge-v6.json");
const CORRUPTED_EQUALITY_RULE_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-rule-v6.json");
const CORRUPTED_EQUALITY_SIDE_CONDITION_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-side-condition-v6.json");
const CORRUPTED_EQUALITY_ANCHOR_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-anchor-v6.json");
const CORRUPTED_EQUALITY_STATUS_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-status-v6.json");
const CORRUPTED_EQUALITY_HASH_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-hash-v6.json");
const CORRUPTED_EQUALITY_EVIDENCE_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-evidence-v6.json");
const CORRUPTED_EQUALITY_ORDER_V6: &[u8] =
    include_bytes!("fixtures/corrupted-equality-event-order-v6.json");
const MINIMAL_V7: &[u8] = include_bytes!("fixtures/minimal-v7.json");
const FRESH_MEMORY_V7: &[u8] = include_bytes!("fixtures/fresh-memory-v7.json");
const FORKED_MEMORY_V7: &[u8] = include_bytes!("fixtures/forked-memory-v7.json");
const REUSED_MEMORY_V7: &[u8] = include_bytes!("fixtures/proved-in-place-reuse-v7.json");
const GUARDED_MEMORY_V7: &[u8] = include_bytes!("fixtures/guarded-memory-reuse-v7.json");
const SEALED_MEMORY_V7: &[u8] = include_bytes!("fixtures/sealed-memory-v7.json");
const EQUALITY_MEMORY_V7: &[u8] = include_bytes!("fixtures/equality-materialized-memory-v7.json");
const CORRUPTED_MEMORY_BUFFER_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-buffer-type-v7.json");
const CORRUPTED_MEMORY_LAYOUT_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-layout-v7.json");
const CORRUPTED_MEMORY_LIFETIME_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-lifetime-v7.json");
const CORRUPTED_MEMORY_ALIAS_V7: &[u8] = include_bytes!("fixtures/corrupted-memory-alias-v7.json");
const CORRUPTED_MEMORY_REUSE_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-reuse-certificate-v7.json");
const CORRUPTED_MEMORY_GUARD_V7: &[u8] = include_bytes!("fixtures/corrupted-memory-guard-v7.json");
const CORRUPTED_MEMORY_FALLBACK_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-fallback-v7.json");
const CORRUPTED_MEMORY_HASH_V7: &[u8] = include_bytes!("fixtures/corrupted-memory-hash-v7.json");
const CORRUPTED_MEMORY_ORDER_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-event-order-v7.json");
const CORRUPTED_MEMORY_ALLOCATOR_V7: &[u8] =
    include_bytes!("fixtures/corrupted-memory-allocator-v7.json");
const FUTURE_V8: &[u8] = include_bytes!("fixtures/future-v8.json");

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

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(
        String::with_capacity(digest.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        },
    )
}

#[test]
fn immutable_legacy_fixture_bytes_are_pinned() {
    assert_eq!(
        hash_bytes(MINIMAL_V1),
        "6179d2f90d204e51fcbc237f51a4f8695af3f0908d6ac4759b46eab57d3399db"
    );
    assert_eq!(
        hash_bytes(SAXPY_V1),
        "b235874897a03e822cdd377177023a171b2cdc52a4ca849dca6a37ebd85f749a"
    );
    assert_eq!(
        hash_bytes(MINIMAL_V2),
        "1e8a5a04317a5e3fbcc96fbd25ccc9b733b52ad15254aa30f98244ac9c8e8b4c"
    );
    assert_eq!(
        hash_bytes(MINIMAL_V3),
        "b929554e6b5981695fead2fd5b2fa9425f1718ed41eeab3ce6e83252836a9983"
    );
    assert_eq!(
        hash_bytes(SAXPY_V3),
        "be0759c745ad4d15c369eb8d4c2a2959fba9d8fa327d59e5afe04abd5c25078e"
    );
    assert_eq!(
        hash_bytes(MIXED_V3),
        "6a53d051ae1244b44cc5b61da1b31c7a54e0287ef9403369d2405f14bdf8fc50"
    );
}

#[test]
fn stage_2b_v5_fixture_bytes_are_pinned() {
    for (bytes, expected) in [
        (
            MINIMAL_V5,
            "315f39f987119285e7e441962515ef1a09bbb384686fbb989e8b49665906bf17",
        ),
        (
            MIGRATED_V4_EXACT_V5,
            "aab676dba60973349f5dca3e8faf6c93439259cf880108d6ec9d03447bb55b3f",
        ),
        (
            SPECULATIVE_OPEN_V5,
            "e713531ffda0dca2625c67db6f53ea950be050c5500f351bd001a53653cd0b9f",
        ),
        (
            RECOGNIZED_V5,
            "1869110f297be26ebea70b0b4f5b2ebb5a54b820dad41542e6ff17e76fef4c78",
        ),
        (
            GUARDED_V5,
            "d898a7b83ce8cc6ec09665fb097205ecd0f9b2e7b132dedbc38d18eef0404d1d",
        ),
        (
            SEALED_GUARDED_V5,
            "3639d7aa6b92e4b067b2fbc8f931d7e31038b1e0e72f8a7fd5f755732c9c5bab",
        ),
        (
            REFUTED_V5,
            "70dec571e21ac4c85c8b144c22e11fe10ec133f40fd6373db4df5131899a91e7",
        ),
        (
            MIXED_CANDIDATE_V5,
            "d898a7b83ce8cc6ec09665fb097205ecd0f9b2e7b132dedbc38d18eef0404d1d",
        ),
        (
            CORRUPTED_PROPOSAL_HASH_V5,
            "6175a11031588ad115f5e2be4cf0b7db72dc11e4ea1839c8720186e9e42348b2",
        ),
        (
            CORRUPTED_FRONTIER_V5,
            "a48ab7fe4df2410ddf6726cba577c12d65cfdac8b58a389bbd50c55814b9a240",
        ),
        (
            CORRUPTED_DEBT_V5,
            "3754bde2dd805d39de0ed28f73e8a617d96dc595a89b0ae3977366fb2e921ddb",
        ),
        (
            CORRUPTED_GUARD_V5,
            "e40df73aefb6d83c097e95ac69c15dba7107ca02c385d41473c3754666962f47",
        ),
        (
            CORRUPTED_FALLBACK_V5,
            "bb0e4601bf4fbe4debffca5b122f5406318af1a4df702808f7de91b795db73b4",
        ),
        (
            CORRUPTED_CANDIDATE_HASH_V2_V5,
            "9b6c7d3d95394e16b926b1332f7556b1636a4eb41949ad08dc7a3703fd51c718",
        ),
        (
            CORRUPTED_CANDIDATE_SEMANTICS_V2_V5,
            "b7fc62546783cf545bf3de8127223143b778ffbc1e68b0b292502a3c8a7a8b2d",
        ),
        (
            FUTURE_V6,
            "d8c46c798c5220c058408d6fb9ead73e6db5919e25a742b960b2a46741cfada0",
        ),
    ] {
        assert_eq!(hash_bytes(bytes), expected);
    }
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
    snapshot: &'a LegacyWorkspaceSnapshotV2,
}

#[derive(Serialize)]
struct BodyV3<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV3,
}

#[derive(Serialize)]
struct BodyV4<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV4,
}

#[derive(Serialize)]
struct BodyV5<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV5,
}

#[derive(Serialize)]
struct BodyV6<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV6,
}

#[derive(Serialize)]
struct BodyV7<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a WorkspaceSnapshot,
}

fn candidate_workspace() -> Workspace {
    let id = WorkspaceId::new("candidate-archive");
    let mut workspace = Workspace::new(id.clone()).unwrap();
    workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateConstant {
                    bind: "$x".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(2),
                },
                Action::CreateConstant {
                    bind: "$y".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(3),
                },
                Action::CreateOp {
                    bind: "$sum".to_owned(),
                    opcode: "add".to_owned(),
                    operands: vec!["$x".to_owned(), "$y".to_owned()],
                    attributes: BTreeMap::new(),
                    region: None,
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$sum".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let rewritten = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate,
            base_revision: identity.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: None,
            }],
        })
        .unwrap();
    workspace
        .candidate_seal(&rewritten.candidate, &rewritten.candidate_revision)
        .unwrap();
    workspace
}

#[test]
fn archive_round_trip_writes_v7_replays_and_resumes_ids() {
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
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V7_NOOP]);
    assert!(
        expected
            .events
            .iter()
            .all(|event| event.semantics_version == CORE_SEMANTICS_VERSION)
    );

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
    assert_eq!(migrated.format_version, LEGACY_ARCHIVE_FORMAT_V2);
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
    assert_eq!(
        loaded.migration.applied_steps,
        [
            MIGRATION_V1_TO_V2,
            MIGRATION_V2_TO_V3,
            MIGRATION_V3_TO_V4,
            MIGRATION_V4_TO_V5,
            MIGRATION_V5_TO_V6,
            MIGRATION_V6_TO_V7,
        ]
    );
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
fn saving_a_loaded_v1_workspace_writes_only_v7() {
    let destination = TestArchive::new("save-migrated");
    let loaded = load_workspace_bytes(MINIMAL_V1).expect("v1 load");
    let saved = save_workspace(destination.path(), &loaded.workspace).expect("v3 save");
    assert_eq!(saved.format_version, ARCHIVE_FORMAT_VERSION);
    let document: Value =
        serde_json::from_slice(&fs::read(destination.path()).expect("read")).expect("JSON");
    assert_eq!(document["format_version"], ARCHIVE_FORMAT_VERSION);
    assert_eq!(document["snapshot"]["schema_version"], 7);
    assert_eq!(
        document["snapshot"]["events"][0]["semantics_version"],
        LEGACY_CORE_SEMANTICS_VERSION
    );
}

#[test]
fn golden_v2_load_runs_explicit_v2_to_v3_migration() {
    let loaded = load_workspace_bytes(MINIMAL_V2).expect("v2 load");
    assert_eq!(loaded.metadata.format_version, LEGACY_ARCHIVE_FORMAT_V2);
    assert_eq!(loaded.migration.source_archive_version, 2);
    assert_eq!(
        loaded.migration.target_archive_version,
        ARCHIVE_FORMAT_VERSION
    );
    assert_eq!(
        loaded.migration.applied_steps,
        [
            MIGRATION_V2_TO_V3,
            MIGRATION_V3_TO_V4,
            MIGRATION_V4_TO_V5,
            MIGRATION_V5_TO_V6,
            MIGRATION_V6_TO_V7,
        ]
    );
    assert!(
        loaded
            .workspace
            .snapshot()
            .events
            .iter()
            .all(|event| event.semantics_version == LEGACY_CORE_SEMANTICS_VERSION)
    );
}

#[test]
fn corrupted_and_future_fixtures_are_rejected_at_the_versioned_boundary() {
    let checksum = load_workspace_bytes(CORRUPTED_V1).expect_err("checksum fails");
    assert_eq!(checksum.code, ErrorCode::PersistenceIntegrity);
    let malformed_v3 = load_workspace_bytes(FUTURE_V3).expect_err("malformed v3 fails");
    assert_eq!(malformed_v3.code, ErrorCode::PersistenceFormat);
    let malformed_v4 = load_workspace_bytes(FUTURE_V4).expect_err("malformed v4 fails");
    assert_eq!(malformed_v4.code, ErrorCode::PersistenceFormat);
    let future = load_workspace_bytes(FUTURE_V5).expect_err("future version fails");
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
        ARCHIVE_FORMAT_VERSION
    );

    let in_place = migrate_archive(source.path(), source.path(), true).expect("in-place migration");
    assert_eq!(in_place.source_archive_version, 1);
    assert_eq!(
        load_workspace(source.path())
            .expect("in-place result")
            .metadata
            .format_version,
        ARCHIVE_FORMAT_VERSION
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
fn golden_v3_fixtures_cover_minimal_saxpy_and_mixed_history() {
    let minimal = load_workspace_bytes(MINIMAL_V3).expect("minimal v3");
    assert_eq!(minimal.metadata.format_version, LEGACY_ARCHIVE_FORMAT_V3);
    assert_eq!(
        minimal.migration.applied_steps,
        [
            MIGRATION_V3_TO_V4,
            MIGRATION_V4_TO_V5,
            MIGRATION_V5_TO_V6,
            MIGRATION_V6_TO_V7,
        ]
    );
    assert!(minimal.workspace.snapshot().events.is_empty());

    let saxpy = load_workspace_bytes(SAXPY_V3).expect("SAXPY v3");
    assert!(
        saxpy
            .workspace
            .snapshot()
            .events
            .iter()
            .all(|event| event.semantics_version == CORE_SEMANTICS_VERSION)
    );
    let program = &saxpy
        .workspace
        .revision(&RevisionId::new("r2"))
        .unwrap()
        .program;
    let evaluated = agentir_eval::evaluate(
        program,
        &BTreeMap::from([
            ("a".to_owned(), json!(2.0)),
            ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
            ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
        ]),
    )
    .unwrap();
    assert_eq!(evaluated.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));

    let mixed = load_workspace_bytes(MIXED_V3).expect("mixed v3");
    let versions = mixed
        .workspace
        .snapshot()
        .events
        .iter()
        .map(|event| event.semantics_version)
        .collect::<Vec<_>>();
    assert_eq!(
        versions,
        [
            LEGACY_CORE_SEMANTICS_VERSION,
            LEGACY_CORE_SEMANTICS_VERSION,
            CORE_SEMANTICS_VERSION,
        ]
    );
    let current = agentir_store::encode_workspace_archive(&mixed.workspace).unwrap();
    let current_json: Value = serde_json::from_slice(&current).unwrap();
    assert_eq!(current_json["format_version"], ARCHIVE_FORMAT_VERSION);
}

#[test]
fn pure_v2_to_v3_migration_tags_legacy_events() {
    let archive: WorkspaceArchiveV2 = serde_json::from_slice(MINIMAL_V2).unwrap();
    let migrated = migrate_archive_v2_to_v3(archive).expect("v2 to v3");
    assert_eq!(migrated.format_version, LEGACY_ARCHIVE_FORMAT_V3);
    assert_eq!(migrated.snapshot.schema_version, 3);
    assert!(
        migrated
            .snapshot
            .events
            .iter()
            .all(|event| event.semantics_version == LEGACY_CORE_SEMANTICS_VERSION)
    );
}

#[test]
fn unsupported_event_semantics_is_rejected_after_valid_checksum() {
    let archive: WorkspaceArchiveV3 =
        serde_json::from_slice(CORRUPTED_SEMANTICS_V3).expect("fixture codec");
    assert_eq!(
        archive.archive_hash,
        hash_body(&BodyV3 {
            format: &archive.format,
            format_version: archive.format_version,
            compiler_version: &archive.compiler_version,
            snapshot: &archive.snapshot,
        })
    );
    let error = load_workspace_bytes(CORRUPTED_SEMANTICS_V3).expect_err("semantics fails");
    assert_eq!(error.code, ErrorCode::PersistenceFormat);
    assert_eq!(error.details["semantics_version"], json!(99));
}

fn archive_mutation_sequence(seed: u64) -> Vec<String> {
    let fixtures = [
        MINIMAL_V1,
        MINIMAL_V2,
        MINIMAL_V3,
        SAXPY_V3,
        MIXED_V3,
        MINIMAL_V4,
        SAXPY_FROZEN_V4,
        SAXPY_IDENTITY_V4,
        REWRITTEN_SEALED_V4,
        MIGRATED_CANDIDATE_V4,
        MINIMAL_V5,
        SPECULATIVE_OPEN_V5,
        RECOGNIZED_V5,
        GUARDED_V5,
        REFUTED_V5,
        MIXED_CANDIDATE_V5,
        MINIMAL_V6,
        EQUALITY_ROOT_V6,
        EQUALITY_SATURATED_V6,
        EQUALITY_MATERIALIZED_V6,
        MINIMAL_V7,
        FRESH_MEMORY_V7,
        FORKED_MEMORY_V7,
        REUSED_MEMORY_V7,
        GUARDED_MEMORY_V7,
        SEALED_MEMORY_V7,
        EQUALITY_MEMORY_V7,
    ];
    let mut state = seed;
    let mut results = Vec::new();
    for case in 0..96 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let fixture = fixtures[case % fixtures.len()];
        let mut mutated = fixture.to_vec();
        let index = usize::try_from(state).unwrap() % mutated.len();
        mutated[index] ^= 1_u8 << (state % 7);
        results.push(match load_workspace_bytes(&mutated) {
            Ok(_) => "OK".to_owned(),
            Err(error) => format!("{:?}", error.code),
        });
    }
    results
}

#[test]
fn fixed_seed_archive_mutation_corpus_is_panic_free_and_reproducible() {
    assert_eq!(
        archive_mutation_sequence(0xa11ce),
        archive_mutation_sequence(0xa11ce)
    );
}

#[test]
fn allocator_counter_tampering_is_rejected_during_replay() {
    let mut archive: WorkspaceArchiveV3 = serde_json::from_slice(SAXPY_V3).unwrap();
    let mut snapshot = serde_json::to_value(&archive.snapshot).unwrap();
    snapshot["allocator"]["value"] = json!(999);
    archive.snapshot = serde_json::from_value(snapshot).unwrap();
    archive.archive_hash = hash_body(&BodyV3 {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    });
    let bytes = serde_json::to_vec(&archive).unwrap();
    let error = load_workspace_bytes(&bytes).expect_err("allocator mismatch");
    assert_eq!(error.code, ErrorCode::ReplayMismatch);
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

#[test]
fn v6_candidate_history_round_trips_byte_deterministically() {
    let workspace = candidate_workspace();
    let first = agentir_store::encode_workspace_archive(&workspace).unwrap();
    let archive: WorkspaceArchiveV7 = serde_json::from_slice(&first).unwrap();
    assert_eq!(archive.format_version, ARCHIVE_FORMAT_VERSION);
    assert_eq!(archive.snapshot.schema_version, 7);
    assert_eq!(archive.snapshot.candidate_forest.candidates.len(), 1);
    assert_eq!(archive.snapshot.candidate_forest.events.len(), 3);
    let loaded = load_workspace_bytes(&first).expect("candidate archive replay");
    assert_eq!(loaded.replay.candidates_verified, 1);
    assert_eq!(loaded.replay.candidate_events_replayed, 3);
    assert_eq!(loaded.replay.evidence_records_verified, 3);
    let second = agentir_store::encode_workspace_archive(&loaded.workspace).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        loaded
            .workspace
            .candidate_query(&CandidateId::new("c1"))
            .unwrap()
            .head,
        CandidateRevisionId::new("cr3")
    );
}

#[test]
fn golden_v4_fixtures_cover_empty_frozen_identity_rewrite_and_migration() {
    for (bytes, candidates, candidate_events) in [
        (MINIMAL_V4, 0, 0),
        (SAXPY_FROZEN_V4, 0, 0),
        (SAXPY_IDENTITY_V4, 1, 1),
        (REWRITTEN_SEALED_V4, 1, 3),
        (MIGRATED_CANDIDATE_V4, 1, 1),
    ] {
        let loaded = load_workspace_bytes(bytes).expect("valid golden v4 fixture");
        assert_eq!(loaded.metadata.format_version, LEGACY_ARCHIVE_FORMAT_V4);
        assert_eq!(loaded.replay.candidates_verified, candidates);
        assert_eq!(loaded.replay.candidate_events_replayed, candidate_events);
        let migrated = agentir_store::encode_workspace_archive(&loaded.workspace).unwrap();
        let replayed = load_workspace_bytes(&migrated).unwrap();
        assert_eq!(
            agentir_store::encode_workspace_archive(&replayed.workspace).unwrap(),
            migrated
        );
    }
    assert_eq!(
        hash_bytes(MINIMAL_V4),
        "2975f4a4be4977b182a52a46b5b9e4708635a495b0d45ff901b96eaabff467da"
    );
    assert_eq!(
        hash_bytes(SAXPY_FROZEN_V4),
        "0b3ba9b5bed36ea2bd5eb1211a7dc1264ae38e1ac29691bf3218e2656ea7c354"
    );
    assert_eq!(
        hash_bytes(SAXPY_IDENTITY_V4),
        "4c3defbf034202db5f9c1587b14b7d4443cf86d1596ea89dca41d0ef3149b22c"
    );
    assert_eq!(
        hash_bytes(REWRITTEN_SEALED_V4),
        "7231bff4e9e13e9efe84f33cf0ad309b56b859f80c86d80900bd447d0df25d5b"
    );
    assert_eq!(
        hash_bytes(MIGRATED_CANDIDATE_V4),
        "3fc4e8df8c4f5cd5d2ec56a1af28fbe28ba1f4dd2128e1a501030a54002cec0a"
    );
}

#[test]
fn v4_to_v5_preserves_legacy_candidate_ids_hashes_state_and_evidence() {
    let source: WorkspaceArchiveV4 = serde_json::from_slice(REWRITTEN_SEALED_V4).unwrap();
    let loaded = load_workspace_bytes(REWRITTEN_SEALED_V4).unwrap();
    assert!(loaded.workspace.candidate_forest().proposals.is_empty());
    assert_eq!(
        loaded.workspace.candidate_forest().events.len(),
        source.snapshot.candidate_forest.events.len()
    );
    assert_eq!(
        loaded
            .workspace
            .candidate_forest()
            .evidence
            .keys()
            .collect::<Vec<_>>(),
        source
            .snapshot
            .candidate_forest
            .evidence
            .keys()
            .collect::<Vec<_>>()
    );
    for (candidate_id, legacy_candidate) in &source.snapshot.candidate_forest.candidates {
        let current = &loaded.workspace.candidate_forest().candidates[candidate_id];
        assert_eq!(current.id, legacy_candidate.id);
        assert_eq!(current.head, legacy_candidate.head);
        for (revision_id, legacy_revision) in &legacy_candidate.revisions {
            let revision = &current.revisions[revision_id];
            assert_eq!(revision.candidate_hash_version, 1);
            assert_eq!(revision.candidate_hash, legacy_revision.candidate_hash);
            assert_eq!(revision.impl_hash, legacy_revision.impl_hash);
            assert_eq!(revision.evidence, legacy_revision.evidence);
            assert_eq!(
                serde_json::to_value(revision.state).unwrap(),
                serde_json::to_value(legacy_revision.state).unwrap()
            );
            assert!(revision.proof_debt.is_empty());
            assert!(revision.guarded_fallback.is_none());
        }
    }
}

#[test]
fn golden_v4_candidate_corruption_fixtures_pass_envelope_then_fail_replay() {
    for bytes in [
        CORRUPTED_CANDIDATE_SEMANTICS_V4,
        CORRUPTED_IMPL_HASH_V4,
        CORRUPTED_CANDIDATE_HASH_V4,
        CORRUPTED_SPEC_ANCHOR_V4,
        CORRUPTED_EVIDENCE_CHAIN_V4,
    ] {
        let archive: WorkspaceArchiveV4 =
            serde_json::from_slice(bytes).expect("exact v4 source codec");
        assert_eq!(
            archive.archive_hash,
            hash_body(&BodyV4 {
                format: &archive.format,
                format_version: archive.format_version,
                compiler_version: &archive.compiler_version,
                snapshot: &archive.snapshot,
            }),
            "fixture corruption must be inside a valid v4 envelope"
        );
        assert!(
            load_workspace_bytes(bytes).is_err(),
            "candidate corruption must prevent publication"
        );
    }
}

#[test]
fn golden_v5_fixtures_round_trip_exact_speculative_guarded_and_refuted_histories() {
    for bytes in [
        MINIMAL_V5,
        MIGRATED_V4_EXACT_V5,
        SPECULATIVE_OPEN_V5,
        RECOGNIZED_V5,
        GUARDED_V5,
        SEALED_GUARDED_V5,
        REFUTED_V5,
        MIXED_CANDIDATE_V5,
    ] {
        let loaded = load_workspace_bytes(bytes).expect("valid golden v5 fixture");
        assert_eq!(loaded.metadata.format_version, LEGACY_ARCHIVE_FORMAT_V5);
        assert_eq!(
            loaded.migration.applied_steps,
            [MIGRATION_V5_TO_V6, MIGRATION_V6_TO_V7]
        );
        assert!(loaded.workspace.equality_store().spaces.is_empty());
        let current = agentir_store::encode_workspace_archive(&loaded.workspace).unwrap();
        let replayed = load_workspace_bytes(&current).unwrap();
        assert_eq!(
            agentir_store::encode_workspace_archive(&replayed.workspace).unwrap(),
            current
        );
    }
    let mixed = load_workspace_bytes(MIXED_CANDIDATE_V5).unwrap();
    assert_eq!(mixed.replay.candidate_events_replayed, 3);
    let semantics = mixed
        .workspace
        .candidate_forest()
        .events
        .iter()
        .map(|event| event.semantics_version)
        .collect::<Vec<_>>();
    assert_eq!(semantics, [1, 2, 2]);
}

#[test]
fn corrupted_v5_fixtures_have_valid_envelopes_and_fail_before_publication() {
    for bytes in [
        CORRUPTED_PROPOSAL_HASH_V5,
        CORRUPTED_FRONTIER_V5,
        CORRUPTED_DEBT_V5,
        CORRUPTED_GUARD_V5,
        CORRUPTED_FALLBACK_V5,
        CORRUPTED_CANDIDATE_HASH_V2_V5,
        CORRUPTED_CANDIDATE_SEMANTICS_V2_V5,
    ] {
        let archive: WorkspaceArchiveV5 =
            serde_json::from_slice(bytes).expect("exact v5 source codec");
        assert_eq!(
            archive.archive_hash,
            hash_body(&BodyV5 {
                format: &archive.format,
                format_version: archive.format_version,
                compiler_version: &archive.compiler_version,
                snapshot: &archive.snapshot,
            })
        );
        assert!(load_workspace_bytes(bytes).is_err());
    }
    assert_eq!(
        load_workspace_bytes(FUTURE_V6).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
}

#[test]
fn golden_v6_fixtures_replay_root_expansion_merge_discharge_and_materialization() {
    for bytes in [
        MINIMAL_V6,
        EQUALITY_ROOT_V6,
        EQUALITY_PARTIAL_V6,
        EQUALITY_SATURATED_V6,
        EQUALITY_MERGED_V6,
        EQUALITY_DISCHARGED_V6,
        EQUALITY_MATERIALIZED_V6,
        MIXED_CANDIDATE_V6,
    ] {
        let loaded = load_workspace_bytes(bytes).expect("valid golden v6 fixture");
        assert_eq!(loaded.metadata.format_version, 6);
        assert_eq!(loaded.migration.applied_steps, [MIGRATION_V6_TO_V7]);
        let current = agentir_store::encode_workspace_archive(&loaded.workspace).unwrap();
        assert_eq!(
            load_workspace_bytes(&current)
                .unwrap()
                .metadata
                .format_version,
            7
        );
    }
    let mixed = load_workspace_bytes(MIXED_CANDIDATE_V6).unwrap();
    let semantics = mixed
        .workspace
        .candidate_forest()
        .events
        .iter()
        .map(|event| event.semantics_version)
        .collect::<Vec<_>>();
    assert_eq!(semantics, [1, 2, 3]);
    assert_eq!(mixed.replay.equality_spaces_verified, 1);
    assert_eq!(mixed.replay.equality_events_replayed, 3);
}

#[test]
fn corrupted_v6_equality_fixtures_have_valid_envelopes_and_fail_replay() {
    for bytes in [
        CORRUPTED_EQUALITY_NODE_V6,
        CORRUPTED_EQUALITY_EDGE_V6,
        CORRUPTED_EQUALITY_RULE_V6,
        CORRUPTED_EQUALITY_SIDE_CONDITION_V6,
        CORRUPTED_EQUALITY_ANCHOR_V6,
        CORRUPTED_EQUALITY_STATUS_V6,
        CORRUPTED_EQUALITY_HASH_V6,
        CORRUPTED_EQUALITY_EVIDENCE_V6,
        CORRUPTED_EQUALITY_ORDER_V6,
    ] {
        let archive: WorkspaceArchiveV6 =
            serde_json::from_slice(bytes).expect("exact v6 source codec");
        assert_eq!(
            archive.archive_hash,
            hash_body(&BodyV6 {
                format: &archive.format,
                format_version: archive.format_version,
                compiler_version: &archive.compiler_version,
                snapshot: &archive.snapshot,
            })
        );
        assert!(load_workspace_bytes(bytes).is_err());
    }
    let corrupted: WorkspaceArchiveV6 = serde_json::from_slice(CORRUPTED_EQUALITY_HASH_V6).unwrap();
    assert!(
        migrate_archive_v6_to_v7(corrupted).is_err(),
        "the explicit v6 to v7 boundary must replay-verify legacy state"
    );
    assert_eq!(
        load_workspace_bytes(FUTURE_V6).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
    assert_eq!(
        load_workspace_bytes(FUTURE_V7).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
}

#[test]
fn golden_v7_memory_fixtures_replay_exact_physical_histories() {
    for bytes in [
        MINIMAL_V7,
        FRESH_MEMORY_V7,
        FORKED_MEMORY_V7,
        REUSED_MEMORY_V7,
        GUARDED_MEMORY_V7,
        SEALED_MEMORY_V7,
        EQUALITY_MEMORY_V7,
    ] {
        let loaded = load_workspace_bytes(bytes).expect("valid golden v7 fixture");
        assert_eq!(loaded.metadata.format_version, 7);
        assert_eq!(loaded.migration.applied_steps, [MIGRATION_V7_NOOP]);
        assert_eq!(
            agentir_store::encode_workspace_archive(&loaded.workspace).unwrap(),
            bytes
        );
    }
    assert_eq!(
        load_workspace_bytes(FRESH_MEMORY_V7)
            .unwrap()
            .replay
            .memory_events_replayed,
        1
    );
    assert_eq!(
        load_workspace_bytes(FORKED_MEMORY_V7)
            .unwrap()
            .replay
            .memory_plans_verified,
        2
    );
}

#[test]
fn corrupted_and_future_v7_memory_fixtures_are_rejected_without_publication() {
    for bytes in [
        CORRUPTED_MEMORY_BUFFER_V7,
        CORRUPTED_MEMORY_LAYOUT_V7,
        CORRUPTED_MEMORY_LIFETIME_V7,
        CORRUPTED_MEMORY_ALIAS_V7,
        CORRUPTED_MEMORY_REUSE_V7,
        CORRUPTED_MEMORY_GUARD_V7,
        CORRUPTED_MEMORY_FALLBACK_V7,
        CORRUPTED_MEMORY_HASH_V7,
        CORRUPTED_MEMORY_ORDER_V7,
        CORRUPTED_MEMORY_ALLOCATOR_V7,
    ] {
        let archive: WorkspaceArchiveV7 =
            serde_json::from_slice(bytes).expect("exact v7 source codec");
        assert_eq!(
            archive.archive_hash,
            hash_body(&BodyV7 {
                format: &archive.format,
                format_version: archive.format_version,
                compiler_version: &archive.compiler_version,
                snapshot: &archive.snapshot,
            })
        );
        assert!(load_workspace_bytes(bytes).is_err());
    }
    assert_eq!(
        load_workspace_bytes(FUTURE_V7).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
    assert_eq!(
        load_workspace_bytes(FUTURE_V8).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
}

#[test]
fn equality_roots_accept_only_unconditional_exact_candidate_revisions() {
    for bytes in [
        SPECULATIVE_OPEN_V5,
        REFUTED_V5,
        GUARDED_V5,
        SEALED_GUARDED_V5,
    ] {
        let mut loaded = load_workspace_bytes(bytes).expect("valid legacy candidate fixture");
        let candidate = CandidateId::new("c1");
        let head = loaded
            .workspace
            .candidate_query(&candidate)
            .unwrap()
            .head
            .clone();
        assert!(loaded.workspace.equality_create(&candidate, &head).is_err());
    }

    let mut unsupported =
        load_workspace_bytes(SPECULATIVE_OPEN_V5).expect("valid open speculative fixture");
    let candidate = CandidateId::new("c1");
    let open = unsupported
        .workspace
        .candidate_query(&candidate)
        .unwrap()
        .head
        .clone();
    let checked = unsupported
        .workspace
        .candidate_translation_check(&candidate, &open, &agentir_core::ids::ProposalId::new("p1"))
        .expect("unsupported validation is persisted non-fatally");
    assert!(
        unsupported
            .workspace
            .equality_create(&candidate, &checked.candidate.candidate_revision)
            .is_err()
    );

    let mut exact = load_workspace_bytes(REWRITTEN_SEALED_V4).expect("sealed exact fixture");
    let candidate = CandidateId::new("c1");
    let head = exact
        .workspace
        .candidate_query(&candidate)
        .unwrap()
        .head
        .clone();
    exact
        .workspace
        .equality_create(&candidate, &head)
        .expect("a sealed unconditional exact candidate is a valid equality root");
}

#[test]
fn stage_2c_v6_fixture_bytes_are_pinned() {
    for (bytes, expected) in [
        (
            MINIMAL_V6,
            "37b3ce979c93cc55e4ac78b5d85be8639eded96ef43af98ffde24f8bd2f53e7f",
        ),
        (
            EQUALITY_ROOT_V6,
            "bbba60ec35a09843d348e561642ad93a2037bb4d675ada8774c40dfee6eddc4f",
        ),
        (
            EQUALITY_PARTIAL_V6,
            "f25120f683dbb494598188e83850bcba176ad81ccb2db892530b23ec2980ca1a",
        ),
        (
            EQUALITY_SATURATED_V6,
            "bce5c6523ff3cdcdc0923a4494b9d5253e2552a3d642c994bca1482ba890a61d",
        ),
        (
            EQUALITY_MERGED_V6,
            "fb99d73dde2b74d7054ee35acc5f8dc1a833a3f7160f583b78e23c24b2a392e7",
        ),
        (
            EQUALITY_DISCHARGED_V6,
            "2ba14055d37f4f2a8c401adfa18521eb407dd4c11e2b41fec61ada13b2b88b89",
        ),
        (
            EQUALITY_MATERIALIZED_V6,
            "b76a27e44f8d1c01206229aa43a385fc3c41457f8ec22c5fc3482afa3be36830",
        ),
        (
            MIXED_CANDIDATE_V6,
            "96903437b4991d72454c6ef18d830d7433416c59b3623966210e7c4ecd77bdd4",
        ),
        (
            CORRUPTED_EQUALITY_NODE_V6,
            "f62ade68eb89b50623d827c3253bc6b01d419e862d6e7868ec6ebb40a4ff9d63",
        ),
        (
            CORRUPTED_EQUALITY_EDGE_V6,
            "8ff0806080772e34a55df425ac60ec8c9e7cda5930c00c1d853aae7e1c51303a",
        ),
        (
            CORRUPTED_EQUALITY_RULE_V6,
            "8bfa3a42507c16eb1f4e70866ec507aa0741df41db99e5c3afaf2a6120f9326b",
        ),
        (
            CORRUPTED_EQUALITY_SIDE_CONDITION_V6,
            "e9890411ed4f403808654ccf2c52ed51cf1690a72ee3316315b343db6d7205e8",
        ),
        (
            CORRUPTED_EQUALITY_ANCHOR_V6,
            "aa85fb2017b0bb52eea7817cce661e64486f10e8e91967d5fbc122021306ac5c",
        ),
        (
            CORRUPTED_EQUALITY_STATUS_V6,
            "331aebacc87a8c06a87c4c5e38fe6f0c5d49b22d672d3aae6f91902fe8be260e",
        ),
        (
            CORRUPTED_EQUALITY_HASH_V6,
            "75d23557e6a2edf1e77ff6f573f50c980cbd3db2e3dc25a500b6e237b9f82ea0",
        ),
        (
            CORRUPTED_EQUALITY_EVIDENCE_V6,
            "2bccc2767e4d3cae46f440df922e7dfcaae6c1e0e12aec6f8d97d83e62246b31",
        ),
        (
            CORRUPTED_EQUALITY_ORDER_V6,
            "e6d85e42394777dace1189c46629b333e5602bc7907be7f719cb146082658928",
        ),
        (
            FUTURE_V7,
            "279d049cc1519e432388037bc61aba41b37c9eaf6a79587321b581cc801274ed",
        ),
    ] {
        assert_eq!(hash_bytes(bytes), expected);
    }
}

fn rehash_v7(archive: &mut WorkspaceArchiveV7) -> Vec<u8> {
    archive.archive_hash = hash_body(&BodyV7 {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    });
    serde_json::to_vec(archive).unwrap()
}

#[test]
fn v6_candidate_hash_anchor_evidence_and_semantics_corruption_are_rejected() {
    let bytes = agentir_store::encode_workspace_archive(&candidate_workspace()).unwrap();

    let mut impl_hash: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    impl_hash
        .snapshot
        .candidate_forest
        .candidates
        .get_mut(&CandidateId::new("c1"))
        .unwrap()
        .revisions
        .get_mut(&CandidateRevisionId::new("cr3"))
        .unwrap()
        .impl_hash = agentir_core::impl_ir::ImplHash::new("corrupted");
    assert!(load_workspace_bytes(&rehash_v7(&mut impl_hash)).is_err());

    let mut candidate_hash: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    candidate_hash
        .snapshot
        .candidate_forest
        .candidates
        .get_mut(&CandidateId::new("c1"))
        .unwrap()
        .revisions
        .get_mut(&CandidateRevisionId::new("cr3"))
        .unwrap()
        .candidate_hash = agentir_core::candidate::CandidateHash::new("corrupted");
    assert!(load_workspace_bytes(&rehash_v7(&mut candidate_hash)).is_err());

    let mut anchor: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    anchor
        .snapshot
        .candidate_forest
        .candidates
        .get_mut(&CandidateId::new("c1"))
        .unwrap()
        .spec_hash = SpecHash::new("corrupted");
    assert!(load_workspace_bytes(&rehash_v7(&mut anchor)).is_err());

    let mut evidence: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    evidence
        .snapshot
        .candidate_forest
        .candidates
        .get_mut(&CandidateId::new("c1"))
        .unwrap()
        .revisions
        .get_mut(&CandidateRevisionId::new("cr2"))
        .unwrap()
        .proof_chain[1]
        .rule = "corrupted".to_owned();
    assert!(load_workspace_bytes(&rehash_v7(&mut evidence)).is_err());

    let mut semantics: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    semantics.snapshot.candidate_forest.events[0].semantics_version = 99;
    let error = load_workspace_bytes(&rehash_v7(&mut semantics))
        .expect_err("unknown candidate semantics is rejected");
    assert_eq!(error.code, ErrorCode::PersistenceFormat);

    let mut allocator: WorkspaceArchiveV7 = serde_json::from_slice(&bytes).unwrap();
    let mut snapshot = serde_json::to_value(&allocator.snapshot).unwrap();
    snapshot["candidate_forest"]["allocator"]["candidate"] = json!(999);
    allocator.snapshot = serde_json::from_value(snapshot).unwrap();
    let error = load_workspace_bytes(&rehash_v7(&mut allocator))
        .expect_err("candidate allocator tampering is rejected");
    assert_eq!(error.code, ErrorCode::ReplayMismatch);
}
