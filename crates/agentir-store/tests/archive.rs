use agentir_core::{Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId};
use agentir_store::{load_workspace, save_workspace, verify_archive};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

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

#[test]
fn archive_round_trip_replays_and_resumes_ids() {
    let archive = TestArchive::new("round-trip");
    let workspace = simple_workspace();
    let expected = workspace.snapshot();
    let saved = save_workspace(archive.path(), &workspace).expect("saves");
    assert_eq!(saved.revisions, 2);
    assert_eq!(saved.events, 1);

    let mut loaded = load_workspace(archive.path()).expect("loads");
    assert_eq!(loaded.workspace.snapshot(), expected);
    assert_eq!(loaded.replay.revisions_verified, 2);
    assert_eq!(loaded.replay.content_hashes_verified, 2);
    assert_eq!(loaded.metadata.archive_hash, saved.archive_hash);

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
fn archive_checksum_tampering_is_rejected() {
    let archive = TestArchive::new("tamper");
    save_workspace(archive.path(), &simple_workspace()).expect("saves");
    let mut document: Value =
        serde_json::from_slice(&fs::read(archive.path()).expect("reads")).expect("JSON");
    document["archive_hash"] = Value::String("tampered".to_owned());
    fs::write(
        archive.path(),
        serde_json::to_vec(&document).expect("encodes"),
    )
    .expect("writes tampered archive");
    let error = verify_archive(archive.path()).expect_err("tampering fails");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
}
