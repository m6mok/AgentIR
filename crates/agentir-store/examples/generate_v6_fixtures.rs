use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{
        ProposalInput, ProposalOperation, ProposalResult, ProposedImplFragment, RelationKind,
        SpeculativeRewriteProposal,
    },
    equality::EqualityStatus,
    ids::{CandidateId, CandidateRevisionId, EqualityRevisionId, ImplOperationId, ProposalId},
    ir::ConstantValue,
    persistence::LegacyWorkspaceSnapshotV6,
};
use agentir_store::{
    ARCHIVE_KIND, LEGACY_ARCHIVE_FORMAT_V6, WorkspaceArchiveV6, load_workspace_bytes,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

const FIXTURES: &str = "crates/agentir-store/tests/fixtures";

fn transaction(workspace: &Workspace, actions: Vec<Action>) -> Transaction {
    Transaction {
        workspace: workspace.id().clone(),
        base_revision: workspace.head().clone(),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    }
}

fn equality_workspace(id: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
    let build = transaction(
        &workspace,
        vec![
            Action::CreateConstant {
                bind: "$a".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(2),
            },
            Action::CreateConstant {
                bind: "$b".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(3),
            },
            Action::CreateConstant {
                bind: "$c".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(4),
            },
            Action::CreateConstant {
                bind: "$d".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(5),
            },
            Action::CreateOp {
                bind: "$left".to_owned(),
                opcode: "add".to_owned(),
                operands: vec!["$a".to_owned(), "$b".to_owned()],
                attributes: BTreeMap::new(),
                region: None,
            },
            Action::CreateOp {
                bind: "$right".to_owned(),
                opcode: "mul".to_owned(),
                operands: vec!["$c".to_owned(), "$d".to_owned()],
                attributes: BTreeMap::new(),
                region: None,
            },
            Action::CreateOp {
                bind: "$total".to_owned(),
                opcode: "add".to_owned(),
                operands: vec!["$left".to_owned(), "$right".to_owned()],
                attributes: BTreeMap::new(),
                region: None,
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$total".to_owned(),
            },
        ],
    );
    workspace.apply(&build).unwrap();
    let freeze = transaction(&workspace, vec![Action::FreezeSpec]);
    workspace.apply(&freeze).unwrap();
    workspace
}

fn rooted(id: &str) -> Workspace {
    let mut workspace = equality_workspace(id);
    let candidate = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
        .equality_create(&candidate.candidate, &candidate.candidate_revision)
        .unwrap();
    workspace
}

fn expanded(id: &str) -> Workspace {
    let mut workspace = rooted(id);
    let root = workspace
        .equality_query(
            &agentir_core::ids::EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er1"),
        )
        .unwrap();
    workspace
        .equality_expand(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            1,
        )
        .unwrap();
    workspace
}

fn saturated(id: &str) -> Workspace {
    let mut workspace = rooted(id);
    let root = workspace
        .equality_query(
            &agentir_core::ids::EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er1"),
        )
        .unwrap();
    workspace
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            100,
        )
        .unwrap();
    workspace
}

fn discharged(id: &str, seal: bool) -> Workspace {
    let mut workspace = saturated(id);
    let identity = workspace
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap()
        .clone();
    let operands = identity.impl_program.operations[&ImplOperationId::new("iop7")]
        .operands
        .clone();
    let speculative = workspace
        .candidate_propose(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr1"),
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop7"),
                replacement: ProposedImplFragment {
                    inputs: vec![
                        ProposalInput {
                            bind: "$left".to_owned(),
                            value: operands[0].clone(),
                        },
                        ProposalInput {
                            bind: "$right".to_owned(),
                            value: operands[1].clone(),
                        },
                    ],
                    operations: vec![ProposalOperation {
                        bind: "$constant".to_owned(),
                        opcode: "constant".to_owned(),
                        operands: Vec::new(),
                        attributes: BTreeMap::new(),
                        constant: Some(ConstantValue::I32 { value: 25 }),
                        region: None,
                    }],
                    result: ProposalResult {
                        value: "$constant".to_owned(),
                    },
                },
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    let target_hash = speculative.impl_hash;
    let equality = workspace
        .equality_query(
            &agentir_core::ids::EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
        )
        .unwrap();
    let target = workspace
        .equality_store()
        .revision(&equality.equality_space, &equality.equality_revision)
        .unwrap()
        .nodes
        .values()
        .find(|node| node.impl_hash == target_hash)
        .unwrap()
        .id
        .clone();
    let result = workspace
        .candidate_equality_check(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr2"),
            &ProposalId::new("p1"),
            &equality.equality_space,
            &equality.equality_revision,
            &equality.equality_hash,
            &target,
        )
        .unwrap();
    if seal {
        workspace
            .candidate_seal(&CandidateId::new("c1"), &result.candidate_revision)
            .unwrap();
    }
    workspace
}

fn materialized(id: &str) -> Workspace {
    let mut workspace = saturated(id);
    let equality = workspace
        .equality_query(
            &agentir_core::ids::EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
        )
        .unwrap();
    let target = workspace
        .equality_store()
        .revision(&equality.equality_space, &equality.equality_revision)
        .unwrap()
        .nodes
        .keys()
        .next_back()
        .unwrap()
        .clone();
    workspace
        .equality_materialize(
            &equality.equality_space,
            &equality.equality_revision,
            &equality.equality_hash,
            &target,
        )
        .unwrap();
    workspace
}

#[derive(Serialize)]
struct Body<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a LegacyWorkspaceSnapshotV6,
}

fn rehash(archive: &mut WorkspaceArchiveV6) -> Vec<u8> {
    let bytes = serde_json::to_vec(&Body {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    })
    .unwrap();
    let digest = Sha256::digest(bytes);
    let mut hash = String::with_capacity(64);
    for byte in digest {
        write!(hash, "{byte:02x}").unwrap();
    }
    archive.archive_hash = hash;
    let mut bytes = serde_json::to_vec(archive).unwrap();
    bytes.push(b'\n');
    bytes
}

fn deterministic_archive(workspace: &Workspace) -> Vec<u8> {
    let mut snapshot = workspace.snapshot();
    for (index, revision) in snapshot.revisions.values_mut().enumerate() {
        revision.created_at_unix_ms = index as u128;
    }
    let snapshot = LegacyWorkspaceSnapshotV6 {
        schema_version: LEGACY_ARCHIVE_FORMAT_V6,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
    };
    let mut archive = WorkspaceArchiveV6 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: LEGACY_ARCHIVE_FORMAT_V6,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
        archive_hash: String::new(),
    };
    rehash(&mut archive)
}

fn write_fixture(name: &str, bytes: &[u8]) {
    fs::write(Path::new(FIXTURES).join(name), bytes).unwrap();
}

fn write_valid(name: &str, workspace: &Workspace) -> Vec<u8> {
    let bytes = deterministic_archive(workspace);
    load_workspace_bytes(&bytes).unwrap();
    write_fixture(name, &bytes);
    bytes
}

fn write_corrupted(name: &str, bytes: &[u8], mutate: impl FnOnce(&mut WorkspaceArchiveV6)) {
    let mut archive: WorkspaceArchiveV6 = serde_json::from_slice(bytes).unwrap();
    mutate(&mut archive);
    let corrupted = rehash(&mut archive);
    assert!(load_workspace_bytes(&corrupted).is_err());
    write_fixture(name, &corrupted);
}

fn equality_revision_mut<'a>(
    archive: &'a mut WorkspaceArchiveV6,
    revision: &str,
) -> &'a mut agentir_core::equality::EqualityRevision {
    archive
        .snapshot
        .equality_store
        .spaces
        .get_mut(&agentir_core::ids::EqualitySpaceId::new("eqs1"))
        .unwrap()
        .revisions
        .get_mut(&EqualityRevisionId::new(revision))
        .unwrap()
}

fn main() {
    let minimal = Workspace::new(WorkspaceId::new("fixture-minimal-v6")).unwrap();
    write_valid("minimal-v6.json", &minimal);
    let root = write_valid("equality-root-v6.json", &rooted("fixture-root-v6"));
    let partial = write_valid(
        "equality-partially-expanded-v6.json",
        &expanded("fixture-expanded-v6"),
    );
    let saturated_bytes = write_valid(
        "equality-saturated-v6.json",
        &saturated("fixture-saturated-v6"),
    );
    write_valid(
        "equality-merged-paths-v6.json",
        &saturated("fixture-merged-v6"),
    );
    let discharged_bytes = write_valid(
        "equality-discharged-v6.json",
        &discharged("fixture-discharged-v6", false),
    );
    write_valid(
        "equality-materialized-v6.json",
        &materialized("fixture-materialized-v6"),
    );
    write_valid(
        "mixed-candidate-semantics-v6.json",
        &discharged("fixture-mixed-v6", true),
    );

    write_corrupted(
        "corrupted-equality-node-hash-v6.json",
        &saturated_bytes,
        |archive| {
            equality_revision_mut(archive, "er2")
                .nodes
                .get_mut(&agentir_core::ids::EqualityNodeId::new("en2"))
                .unwrap()
                .impl_hash = agentir_core::impl_ir::ImplHash::new("corrupted");
        },
    );
    write_corrupted(
        "corrupted-equality-edge-v6.json",
        &saturated_bytes,
        |archive| {
            let edge = equality_revision_mut(archive, "er2")
                .edges
                .values_mut()
                .next()
                .unwrap();
            edge.proof_digest.clone_from(&"corrupted".to_owned());
        },
    );
    write_corrupted(
        "corrupted-equality-rule-v6.json",
        &saturated_bytes,
        |archive| {
            let edge = equality_revision_mut(archive, "er2")
                .edges
                .values_mut()
                .next()
                .unwrap();
            edge.descriptor
                .rule
                .clone_from(&"agent_supplied_rule".to_owned());
        },
    );
    write_corrupted(
        "corrupted-equality-side-condition-v6.json",
        &saturated_bytes,
        |archive| {
            equality_revision_mut(archive, "er2")
                .edges
                .values_mut()
                .next()
                .unwrap()
                .descriptor
                .side_conditions = vec!["agent supplied condition".to_owned()];
        },
    );
    write_corrupted("corrupted-equality-anchor-v6.json", &root, |archive| {
        archive
            .snapshot
            .equality_store
            .spaces
            .get_mut(&agentir_core::ids::EqualitySpaceId::new("eqs1"))
            .unwrap()
            .anchor
            .root_impl_hash = agentir_core::impl_ir::ImplHash::new("corrupted");
    });
    write_corrupted("corrupted-equality-status-v6.json", &partial, |archive| {
        equality_revision_mut(archive, "er2").status = EqualityStatus::FixedPoint;
    });
    write_corrupted(
        "corrupted-equality-hash-v6.json",
        &saturated_bytes,
        |archive| {
            equality_revision_mut(archive, "er2").equality_hash =
                agentir_core::equality::EqualityHash::new("corrupted");
        },
    );
    write_corrupted(
        "corrupted-equality-evidence-v6.json",
        &discharged_bytes,
        |archive| {
            let proof = &mut archive
                .snapshot
                .candidate_forest
                .candidates
                .get_mut(&CandidateId::new("c1"))
                .unwrap()
                .revisions
                .get_mut(&CandidateRevisionId::new("cr3"))
                .unwrap()
                .equality_proofs[0];
            proof.path_digest.clone_from(&"corrupted".to_owned());
        },
    );
    write_corrupted(
        "corrupted-equality-event-order-v6.json",
        &saturated_bytes,
        |archive| {
            archive.snapshot.equality_store.events[1].candidate_event_cursor = 999;
        },
    );
    write_fixture(
        "future-v7.json",
        b"{\"format\":\"agentir.workspace\",\"format_version\":7}\n",
    );
}
