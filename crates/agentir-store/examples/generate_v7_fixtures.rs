use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionSpec},
    candidate::RelationKind,
    ids::{
        BufferId, CandidateId, CandidateRevisionId, ImplValueId, MemoryPlanId, MemoryRevisionId,
    },
    memory::{MemoryAction, MemoryHash, MemoryTransaction},
};
use agentir_store::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_KIND, WorkspaceArchiveV7, load_workspace_bytes,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

const FIXTURES: &str = "crates/agentir-store/tests/fixtures";

#[derive(Serialize)]
struct Body<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::WorkspaceSnapshot,
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(output, "{byte:02x}").unwrap();
    }
    output
}

fn rehash(archive: &mut WorkspaceArchiveV7) -> Vec<u8> {
    archive.archive_hash = sha256(
        &serde_json::to_vec(&Body {
            format: &archive.format,
            format_version: archive.format_version,
            compiler_version: &archive.compiler_version,
            snapshot: &archive.snapshot,
        })
        .unwrap(),
    );
    let mut bytes = serde_json::to_vec(archive).unwrap();
    bytes.push(b'\n');
    bytes
}

fn deterministic_archive(workspace: &Workspace) -> Vec<u8> {
    let mut snapshot = workspace.snapshot();
    for (index, revision) in snapshot.revisions.values_mut().enumerate() {
        revision.created_at_unix_ms = index as u128;
    }
    rehash(&mut WorkspaceArchiveV7 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: ARCHIVE_FORMAT_VERSION,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
        archive_hash: String::new(),
    })
}

fn write(name: &str, bytes: &[u8]) {
    fs::write(Path::new(FIXTURES).join(name), bytes).unwrap();
}

fn write_valid(name: &str, workspace: &Workspace) -> Vec<u8> {
    let bytes = deterministic_archive(workspace);
    load_workspace_bytes(&bytes).unwrap();
    write(name, &bytes);
    bytes
}

fn transaction(workspace: &Workspace, actions: Vec<Action>) -> Transaction {
    Transaction {
        workspace: workspace.id().clone(),
        base_revision: workspace.head().clone(),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    }
}

fn memory_workspace(id: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
    let region = || RegionSpec {
        arguments: vec![RegionArgumentSpec {
            name: "element".to_owned(),
            ty: "f32".parse().unwrap(),
        }],
        captures: Vec::new(),
        operations: Vec::new(),
        yield_value: "element".to_owned(),
    };
    let build = transaction(
        &workspace,
        vec![
            Action::DefineDimension {
                bind: Some("$N".to_owned()),
                name: "N".to_owned(),
                constraints: vec!["N >= 0".to_owned()],
            },
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "tensor<f32,[N]>".parse().unwrap(),
            },
            Action::CreateOp {
                bind: "$temporary".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["$x".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(region()),
            },
            Action::CreateOp {
                bind: "$out".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["$temporary".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(region()),
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$out".to_owned(),
            },
        ],
    );
    workspace.apply(&build).unwrap();
    let freeze = transaction(&workspace, vec![Action::FreezeSpec]);
    workspace.apply(&freeze).unwrap();
    workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    workspace
}

fn root_transaction(workspace: &Workspace, action: MemoryAction) -> MemoryTransaction {
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    MemoryTransaction {
        memory_plan: root.memory_plan,
        base_memory_revision: root.memory_revision,
        expected_memory_hash: root.memory_hash,
        expected_impl_hash: root.impl_hash,
        actions: vec![action],
    }
}

fn corrupted(name: &str, source: &[u8], mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(source).unwrap();
    mutate(&mut value);
    let mut archive: WorkspaceArchiveV7 = serde_json::from_value(value).unwrap();
    let bytes = rehash(&mut archive);
    assert!(load_workspace_bytes(&bytes).is_err());
    write(name, &bytes);
}

fn main() {
    write_valid(
        "minimal-v7.json",
        &Workspace::new(WorkspaceId::new("minimal-v7")).unwrap(),
    );

    let fresh = memory_workspace("fresh-memory-v7");
    let fresh_bytes = write_valid("fresh-memory-v7.json", &fresh);
    write("rejected-unsafe-reuse-v7.json", &fresh_bytes);

    let mut forked = fresh.clone();
    let root_hash = forked
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap()
        .memory_hash;
    forked
        .memory_fork(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr1"),
            &root_hash,
        )
        .unwrap();
    write_valid("forked-memory-v7.json", &forked);

    let mut reused = fresh.clone();
    reused
        .memory_apply(&root_transaction(
            &reused,
            MemoryAction::RequestInPlaceReuse {
                input: ImplValueId::new("iv2"),
                result: ImplValueId::new("iv3"),
            },
        ))
        .unwrap();
    let reused_bytes = write_valid("proved-in-place-reuse-v7.json", &reused);

    let mut guarded = fresh.clone();
    guarded
        .memory_apply(&root_transaction(
            &guarded,
            MemoryAction::RequestGuardedReuse {
                input: ImplValueId::new("iv2"),
                result: ImplValueId::new("iv3"),
                guard_against: BufferId::new("buf1"),
            },
        ))
        .unwrap();
    let guarded_bytes = write_valid("guarded-memory-reuse-v7.json", &guarded);
    write("false-guard-fallback-v7.json", &guarded_bytes);

    let mut sealed = reused.clone();
    let reused_head = sealed
        .memory_store()
        .plan(&MemoryPlanId::new("mp1"))
        .unwrap()
        .head
        .clone();
    let reused_hash = sealed
        .memory_query(&MemoryPlanId::new("mp1"), &reused_head)
        .unwrap()
        .memory_hash;
    sealed
        .memory_seal(&MemoryPlanId::new("mp1"), &reused_head, &reused_hash)
        .unwrap();
    write_valid("sealed-memory-v7.json", &sealed);

    let materialized = load_workspace_bytes(include_bytes!(
        "../tests/fixtures/equality-materialized-v6.json"
    ))
    .unwrap()
    .workspace;
    let mut equality_memory = materialized;
    let candidate = equality_memory
        .candidate_forest()
        .candidates
        .last_key_value()
        .unwrap()
        .1;
    let candidate_id = candidate.id.clone();
    let candidate_revision = candidate.head.clone();
    equality_memory
        .memory_create(&candidate_id, &candidate_revision)
        .unwrap();
    let equality_bytes = write_valid("equality-materialized-memory-v7.json", &equality_memory);
    write("mixed-memory-semantics-v7.json", &equality_bytes);

    corrupted(
        "corrupted-memory-buffer-type-v7.json",
        &fresh_bytes,
        |value| {
            value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr1"]["program"]["buffers"]
                ["buf2"]["element_type"] = json!("i32");
        },
    );
    corrupted("corrupted-memory-layout-v7.json", &fresh_bytes, |value| {
        value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr1"]["program"]["buffers"]
            ["buf2"]["strides"]["entries"][0]["value"] = json!(2);
    });
    corrupted("corrupted-memory-lifetime-v7.json", &fresh_bytes, |value| {
        value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr1"]["program"]["buffers"]
            ["buf2"]["lifetime"]["last_use"] = json!(999);
    });
    corrupted("corrupted-memory-alias-v7.json", &fresh_bytes, |value| {
        value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr1"]["program"]["alias_facts"]
            [0]["provenance"] = json!("unverified_claim");
    });
    corrupted(
        "corrupted-memory-reuse-certificate-v7.json",
        &reused_bytes,
        |value| {
            value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr2"]["program"]["reuse_decisions"]
                ["iv3"]["certificate"] = json!("corrupted");
        },
    );
    corrupted("corrupted-memory-guard-v7.json", &guarded_bytes, |value| {
        value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr2"]["program"]["reuse_decisions"]
            ["iv3"]["guard"]["dependencies"] = json!([]);
    });
    corrupted(
        "corrupted-memory-fallback-v7.json",
        &guarded_bytes,
        |value| {
            value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr2"]["program"]["reuse_decisions"]
                ["iv3"]["fallback"]["result"] = json!("iv2");
        },
    );
    corrupted("corrupted-memory-hash-v7.json", &fresh_bytes, |value| {
        value["snapshot"]["memory_store"]["plans"]["mp1"]["revisions"]["mr1"]["memory_hash"] =
            json!(MemoryHash::new("corrupted"));
    });
    corrupted(
        "corrupted-memory-event-order-v7.json",
        &fresh_bytes,
        |value| {
            value["snapshot"]["memory_store"]["events"][0]["candidate_event_cursor"] = json!(999);
        },
    );
    corrupted(
        "corrupted-memory-allocator-v7.json",
        &fresh_bytes,
        |value| {
            value["snapshot"]["memory_store"]["allocator"]["plan"] = json!(999);
        },
    );

    write(
        "future-v8.json",
        b"{\"format\":\"agentir.workspace\",\"format_version\":8,\"compiler_version\":\"0.1.0\",\"snapshot\":{},\"archive_hash\":\"future\"}\n",
    );
}
