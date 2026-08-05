use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{
        ProposalInput, ProposalOperation, ProposalResult, ProposedImplFragment, RelationKind,
        SpeculativeRewriteProposal,
    },
    ids::{CandidateId, CandidateRevisionId, ImplOperationId, ImplValueId, ProposalId},
    impl_ir::ImplHash,
    ir::ConstantValue,
    resources::ResourceLimits,
};
use agentir_store::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_KIND, WorkspaceArchiveV5, load_workspace_bytes,
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

fn freeze(workspace: &mut Workspace) {
    let transaction = transaction(workspace, vec![Action::FreezeSpec]);
    workspace.apply(&transaction).unwrap();
}

fn parameter_add(id: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
    let transaction = transaction(
        &workspace,
        vec![
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "i32".parse().unwrap(),
            },
            Action::CreateParameter {
                bind: "$y".to_owned(),
                name: "y".to_owned(),
                ty: "i32".parse().unwrap(),
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
    );
    workspace.apply(&transaction).unwrap();
    freeze(&mut workspace);
    workspace
}

fn constant_add(id: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
    let transaction = transaction(
        &workspace,
        vec![
            Action::CreateConstant {
                bind: "$x".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(2),
            },
            Action::CreateConstant {
                bind: "$y".to_owned(),
                ty: "i32".parse().unwrap(),
                value: serde_json::json!(3),
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
    );
    workspace.apply(&transaction).unwrap();
    freeze(&mut workspace);
    workspace
}

fn self_division(id: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
    let transaction = transaction(
        &workspace,
        vec![
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "i32".parse().unwrap(),
            },
            Action::CreateOp {
                bind: "$division".to_owned(),
                opcode: "div".to_owned(),
                operands: vec!["$x".to_owned(), "$x".to_owned()],
                attributes: BTreeMap::new(),
                region: None,
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$division".to_owned(),
            },
        ],
    );
    workspace.apply(&transaction).unwrap();
    freeze(&mut workspace);
    workspace
}

fn binary_fragment(opcode: &str) -> ProposedImplFragment {
    ProposedImplFragment {
        inputs: vec![
            ProposalInput {
                bind: "$x".to_owned(),
                value: ImplValueId::new("iv1"),
            },
            ProposalInput {
                bind: "$y".to_owned(),
                value: ImplValueId::new("iv2"),
            },
        ],
        operations: vec![ProposalOperation {
            bind: "$result".to_owned(),
            opcode: opcode.to_owned(),
            operands: vec!["$x".to_owned(), "$y".to_owned()],
            attributes: BTreeMap::new(),
            constant: None,
            region: None,
        }],
        result: ProposalResult {
            value: "$result".to_owned(),
        },
    }
}

fn create_identity(workspace: &mut Workspace) -> agentir_core::candidate::CandidateCheckReport {
    workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap()
}

fn speculative_open() -> Workspace {
    let mut workspace = parameter_add("fixture-speculative-open");
    let identity = create_identity(&mut workspace);
    workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: binary_fragment("sub"),
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    workspace
}

fn recognized() -> Workspace {
    let mut workspace = constant_add("fixture-recognized");
    let identity = create_identity(&mut workspace);
    let accepted = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: ProposedImplFragment {
                    inputs: vec![
                        ProposalInput {
                            bind: "$x".to_owned(),
                            value: ImplValueId::new("iv1"),
                        },
                        ProposalInput {
                            bind: "$y".to_owned(),
                            value: ImplValueId::new("iv2"),
                        },
                    ],
                    operations: vec![ProposalOperation {
                        bind: "$five".to_owned(),
                        opcode: "constant".to_owned(),
                        operands: Vec::new(),
                        attributes: BTreeMap::new(),
                        constant: Some(ConstantValue::I32 { value: 5 }),
                        region: None,
                    }],
                    result: ProposalResult {
                        value: "$five".to_owned(),
                    },
                },
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: false,
                claimed_rule: Some("ignored".to_owned()),
            },
        )
        .unwrap();
    workspace
        .candidate_translation_check(
            &CandidateId::new("c1"),
            &accepted.candidate_revision,
            &ProposalId::new("p1"),
        )
        .unwrap();
    workspace
}

fn guarded(seal: bool) -> Workspace {
    let mut workspace = self_division(if seal {
        "fixture-guarded-sealed"
    } else {
        "fixture-guarded"
    });
    let identity = create_identity(&mut workspace);
    let accepted = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop2"),
                replacement: ProposedImplFragment {
                    inputs: vec![
                        ProposalInput {
                            bind: "$left".to_owned(),
                            value: ImplValueId::new("iv1"),
                        },
                        ProposalInput {
                            bind: "$right".to_owned(),
                            value: ImplValueId::new("iv1"),
                        },
                    ],
                    operations: vec![ProposalOperation {
                        bind: "$one".to_owned(),
                        opcode: "constant".to_owned(),
                        operands: Vec::new(),
                        attributes: BTreeMap::new(),
                        constant: Some(ConstantValue::I32 { value: 1 }),
                        region: None,
                    }],
                    result: ProposalResult {
                        value: "$one".to_owned(),
                    },
                },
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    let translated = workspace
        .candidate_translation_check(
            &CandidateId::new("c1"),
            &accepted.candidate_revision,
            &ProposalId::new("p1"),
        )
        .unwrap();
    if seal {
        workspace
            .candidate_seal(
                &CandidateId::new("c1"),
                &translated.candidate.candidate_revision,
            )
            .unwrap();
    }
    workspace
}

fn refuted() -> Workspace {
    let mut workspace = speculative_open();
    let candidate = CandidateId::new("c1");
    let revision = CandidateRevisionId::new("cr2");
    let spec = workspace
        .revision(&RevisionId::new("r2"))
        .unwrap()
        .program
        .clone();
    let validation = agentir_eval::differential_validate_candidate(
        &spec,
        workspace.candidate_forest(),
        &candidate,
        &revision,
        1,
        32,
        &ResourceLimits::default(),
    )
    .unwrap();
    assert!(!validation.passed);
    workspace
        .candidate_record_validation(&candidate, &revision, validation)
        .unwrap();
    workspace
}

fn write(name: &str, bytes: &[u8]) {
    fs::write(Path::new(FIXTURES).join(name), bytes).unwrap();
}

#[derive(Serialize)]
struct Body<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::WorkspaceSnapshot,
}

fn rehash(archive: &mut WorkspaceArchiveV5) -> Vec<u8> {
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

fn deterministic_archive(workspace: &Workspace, normalize_timestamps: bool) -> Vec<u8> {
    let mut snapshot = workspace.snapshot();
    if normalize_timestamps {
        for (index, revision) in snapshot.revisions.values_mut().enumerate() {
            revision.created_at_unix_ms = index as u128;
        }
    }
    let mut archive = WorkspaceArchiveV5 {
        format: ARCHIVE_KIND.to_owned(),
        format_version: ARCHIVE_FORMAT_VERSION,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot,
        archive_hash: String::new(),
    };
    rehash(&mut archive)
}

fn corrupted(bytes: &[u8], name: &str, mutate: impl FnOnce(&mut WorkspaceArchiveV5)) {
    let mut archive: WorkspaceArchiveV5 = serde_json::from_slice(bytes).unwrap();
    mutate(&mut archive);
    write(name, &rehash(&mut archive));
}

fn main() {
    let minimal = Workspace::new(WorkspaceId::new("fixture-minimal-v5")).unwrap();
    write("minimal-v5.json", &deterministic_archive(&minimal, true));

    let legacy = fs::read(Path::new(FIXTURES).join("saxpy-identity-v4.json")).unwrap();
    let migrated = load_workspace_bytes(&legacy).unwrap();
    write(
        "migrated-v4-exact-v5.json",
        &deterministic_archive(&migrated.workspace, false),
    );

    let open = deterministic_archive(&speculative_open(), true);
    write("speculative-open-v5.json", &open);
    let recognized = deterministic_archive(&recognized(), true);
    write("recognized-known-rewrite-v5.json", &recognized);
    let guarded_bytes = deterministic_archive(&guarded(false), true);
    write("guarded-candidate-v5.json", &guarded_bytes);
    write("mixed-candidate-semantics-v5.json", &guarded_bytes);
    write(
        "sealed-guarded-v5.json",
        &deterministic_archive(&guarded(true), true),
    );
    write(
        "refuted-candidate-v5.json",
        &deterministic_archive(&refuted(), true),
    );

    corrupted(&open, "corrupted-proposal-hash-v5.json", |archive| {
        archive
            .snapshot
            .candidate_forest
            .proposals
            .get_mut(&ProposalId::new("p1"))
            .unwrap()
            .proposal_hash = agentir_core::candidate::ProposalHash::new("corrupted");
    });
    corrupted(&open, "corrupted-proof-frontier-v5.json", |archive| {
        archive
            .snapshot
            .candidate_forest
            .candidates
            .get_mut(&CandidateId::new("c1"))
            .unwrap()
            .revisions
            .get_mut(&CandidateRevisionId::new("cr2"))
            .unwrap()
            .proof_frontier
            .as_mut()
            .unwrap()
            .terminal_proved_impl_hash = ImplHash::new("corrupted");
    });
    corrupted(&open, "corrupted-debt-chain-v5.json", |archive| {
        archive
            .snapshot
            .candidate_forest
            .candidates
            .get_mut(&CandidateId::new("c1"))
            .unwrap()
            .revisions
            .get_mut(&CandidateRevisionId::new("cr2"))
            .unwrap()
            .proof_debt[0]
            .before_impl_hash = ImplHash::new("corrupted");
    });
    corrupted(&guarded_bytes, "corrupted-guard-v5.json", |archive| {
        let fallback = archive
            .snapshot
            .candidate_forest
            .candidates
            .get_mut(&CandidateId::new("c1"))
            .unwrap()
            .revisions
            .get_mut(&CandidateRevisionId::new("cr3"))
            .unwrap()
            .guarded_fallback
            .as_mut()
            .unwrap();
        fallback.guard = agentir_core::candidate::GuardPredicate::I32NonZero {
            value: ImplValueId::new("iv999"),
        };
    });
    corrupted(&guarded_bytes, "corrupted-fallback-v5.json", |archive| {
        archive
            .snapshot
            .candidate_forest
            .candidates
            .get_mut(&CandidateId::new("c1"))
            .unwrap()
            .revisions
            .get_mut(&CandidateRevisionId::new("cr3"))
            .unwrap()
            .guarded_fallback
            .as_mut()
            .unwrap()
            .fallback_revision = CandidateRevisionId::new("cr999");
    });
    corrupted(
        &guarded_bytes,
        "corrupted-candidate-hash-v2-v5.json",
        |archive| {
            archive
                .snapshot
                .candidate_forest
                .candidates
                .get_mut(&CandidateId::new("c1"))
                .unwrap()
                .revisions
                .get_mut(&CandidateRevisionId::new("cr3"))
                .unwrap()
                .candidate_hash = agentir_core::candidate::CandidateHash::new("corrupted");
        },
    );
    corrupted(
        &guarded_bytes,
        "corrupted-candidate-semantics-v2-v5.json",
        |archive| {
            archive.snapshot.candidate_forest.events[1].semantics_version = 99;
        },
    );
    write(
        "future-v6.json",
        b"{\"format\":\"agentir.workspace\",\"format_version\":6}\n",
    );
}
