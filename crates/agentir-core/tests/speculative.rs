use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{
        CandidateState, EquivalenceStatus, ProofDebtStatus, ProposalInput, ProposalOperation,
        ProposalResult, ProposedImplFragment, RelationKind, SpeculativeRewriteProposal,
        TranslationValidationResult, canonicalize_proposal_with_limit,
    },
    ids::{CandidateId, CandidateRevisionId, ImplOperationId, ImplValueId, ProposalId},
    ir::ConstantValue,
    resources::ResourceLimits,
};
use serde_json::json;
use std::collections::BTreeMap;

fn binary_workspace(parameters: bool, opcode: &str) -> Workspace {
    let id = WorkspaceId::new("stage-2b");
    let mut workspace = Workspace::new(id.clone()).unwrap();
    let mut actions = if parameters {
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
        ]
    } else {
        vec![
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
        ]
    };
    actions.extend([
        Action::CreateOp {
            bind: "$result".to_owned(),
            opcode: opcode.to_owned(),
            operands: vec!["$x".to_owned(), "$y".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        },
        Action::SetOutput {
            name: "out".to_owned(),
            value: "$result".to_owned(),
        },
    ]);
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: WorkspaceId::new("stage-2b"),
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
}

fn binary_fragment(opcode: &str, left: &str, right: &str) -> ProposedImplFragment {
    ProposedImplFragment {
        inputs: vec![
            ProposalInput {
                bind: left.to_owned(),
                value: ImplValueId::new("iv1"),
            },
            ProposalInput {
                bind: right.to_owned(),
                value: ImplValueId::new("iv2"),
            },
        ],
        operations: vec![ProposalOperation {
            bind: "$result".to_owned(),
            opcode: opcode.to_owned(),
            operands: vec![left.to_owned(), right.to_owned()],
            attributes: BTreeMap::new(),
            constant: None,
            region: None,
        }],
        result: ProposalResult {
            value: "$result".to_owned(),
        },
    }
}

#[test]
fn unknown_proposal_requires_opt_in_and_rejection_consumes_no_ids() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let proposal = SpeculativeRewriteProposal {
        target: ImplOperationId::new("iop3"),
        replacement: binary_fragment("sub", "$left", "$right"),
        expected_before_impl_hash: identity.impl_hash.clone(),
        allow_speculative: false,
        claimed_rule: Some("agent_claims_exact".to_owned()),
    };
    let before = workspace.snapshot();
    let error = workspace
        .candidate_propose(&identity.candidate, &identity.candidate_revision, &proposal)
        .expect_err("unknown proposal needs opt-in");
    assert_eq!(error.code, ErrorCode::SpeculativeOptInRequired);
    assert_eq!(workspace.snapshot(), before);

    let accepted = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                allow_speculative: true,
                ..proposal
            },
        )
        .unwrap();
    assert_eq!(accepted.candidate_revision, CandidateRevisionId::new("cr2"));
    assert_eq!(accepted.state, CandidateState::Speculative);
    assert_eq!(accepted.equivalence.status, EquivalenceStatus::Open);
    assert_eq!(accepted.proof_debt[0].proposal, ProposalId::new("p1"));
    assert_eq!(accepted.proof_debt[0].status, ProofDebtStatus::Open);
    let record = workspace
        .candidate_proposal_query(&ProposalId::new("p1"))
        .unwrap();
    assert_eq!(record.allocated_operations, [ImplOperationId::new("iop4")]);
}

#[test]
fn proposal_hash_alpha_normalizes_local_bindings() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let fork = workspace
        .candidate_fork(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    let first = SpeculativeRewriteProposal {
        target: ImplOperationId::new("iop3"),
        replacement: binary_fragment("sub", "$a", "$b"),
        expected_before_impl_hash: identity.impl_hash.clone(),
        allow_speculative: true,
        claimed_rule: None,
    };
    workspace
        .candidate_propose(&identity.candidate, &identity.candidate_revision, &first)
        .unwrap();
    workspace
        .candidate_propose(
            &fork.candidate,
            &fork.candidate_revision,
            &SpeculativeRewriteProposal {
                replacement: binary_fragment("sub", "$renamed_left", "$renamed_right"),
                ..first
            },
        )
        .unwrap();
    assert_eq!(
        workspace
            .candidate_proposal_query(&ProposalId::new("p1"))
            .unwrap()
            .proposal_hash,
        workspace
            .candidate_proposal_query(&ProposalId::new("p2"))
            .unwrap()
            .proposal_hash
    );
}

#[test]
fn production_known_rewrite_recognition_promotes_the_frontier() {
    let mut workspace = binary_workspace(false, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let replacement = ProposedImplFragment {
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
    };
    let accepted = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement,
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: false,
                claimed_rule: Some("spoofed_name_is_ignored".to_owned()),
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
    assert!(matches!(
        translated.validation.result,
        TranslationValidationResult::RecognizedKnownRewrite { .. }
    ));
    assert_eq!(translated.candidate.state, CandidateState::Equivalent);
    assert_eq!(
        translated.candidate.equivalence.status,
        EquivalenceStatus::Proved
    );
    assert_eq!(
        translated.candidate.proof_debt[0].status,
        ProofDebtStatus::Proved
    );
}

#[test]
fn malformed_and_over_budget_proposals_are_atomic_and_do_not_consume_ids() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let before = workspace.snapshot();
    let invalid_boundary = SpeculativeRewriteProposal {
        target: ImplOperationId::new("iop3"),
        replacement: ProposedImplFragment {
            inputs: vec![ProposalInput {
                bind: "$only".to_owned(),
                value: ImplValueId::new("iv1"),
            }],
            operations: vec![ProposalOperation {
                bind: "$result".to_owned(),
                opcode: "add".to_owned(),
                operands: vec!["$only".to_owned(), "$only".to_owned()],
                attributes: BTreeMap::new(),
                constant: None,
                region: None,
            }],
            result: ProposalResult {
                value: "$result".to_owned(),
            },
        },
        expected_before_impl_hash: identity.impl_hash.clone(),
        allow_speculative: true,
        claimed_rule: None,
    };
    assert_eq!(
        workspace
            .candidate_propose(
                &identity.candidate,
                &identity.candidate_revision,
                &invalid_boundary,
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidProposal
    );
    assert_eq!(workspace.snapshot(), before);

    workspace.set_resource_limits(ResourceLimits {
        open_proof_debt_obligations: 0,
        ..ResourceLimits::default()
    });
    let error = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: binary_fragment("sub", "$left", "$right"),
                expected_before_impl_hash: identity.impl_hash.clone(),
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProofDebtLimitExceeded);
    assert_eq!(workspace.snapshot(), before);

    workspace.set_resource_limits(ResourceLimits::default());
    workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: binary_fragment("sub", "$left", "$right"),
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    assert!(
        workspace
            .candidate_proposal_query(&ProposalId::new("p1"))
            .is_ok()
    );
}

#[test]
fn unsupported_validation_is_persistent_idempotent_and_blocks_seal() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let accepted = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: binary_fragment("sub", "$left", "$right"),
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: Some("fold_defined_scalar_constants".to_owned()),
            },
        )
        .unwrap();
    let unsupported = workspace
        .candidate_translation_check(
            &identity.candidate,
            &accepted.candidate_revision,
            &ProposalId::new("p1"),
        )
        .unwrap();
    assert_eq!(
        unsupported.diagnostic,
        Some(ErrorCode::TranslationUnsupported)
    );
    assert_eq!(
        unsupported.candidate.proof_debt[0].status,
        ProofDebtStatus::Unsupported
    );
    let repeated = workspace
        .candidate_translation_check(
            &identity.candidate,
            &unsupported.candidate.candidate_revision,
            &ProposalId::new("p1"),
        )
        .unwrap();
    assert_eq!(repeated.validation, unsupported.validation);
    assert_eq!(
        repeated.candidate.candidate_revision,
        unsupported.candidate.candidate_revision
    );
    assert_eq!(
        workspace
            .candidate_seal(
                &identity.candidate,
                &unsupported.candidate.candidate_revision,
            )
            .unwrap_err()
            .code,
        ErrorCode::CandidateHasProofDebt
    );

    let recovered = workspace
        .candidate_fork(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    assert!(recovered.proof_debt.is_empty());
    assert_eq!(recovered.equivalence.status, EquivalenceStatus::Proved);
}

#[test]
fn multiple_speculative_steps_preserve_order_and_frontier() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let first = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop3"),
                replacement: binary_fragment("sub", "$left", "$right"),
                expected_before_impl_hash: identity.impl_hash.clone(),
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    let second = workspace
        .candidate_propose(
            &identity.candidate,
            &first.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop4"),
                replacement: binary_fragment("mul", "$renamed_left", "$renamed_right"),
                expected_before_impl_hash: first.impl_hash.clone(),
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    assert_eq!(second.proof_debt.len(), 2);
    assert_eq!(second.proof_debt[0].proposal, ProposalId::new("p1"));
    assert_eq!(second.proof_debt[1].proposal, ProposalId::new("p2"));
    assert_eq!(
        second.proof_debt[0].after_impl_hash,
        second.proof_debt[1].before_impl_hash
    );
    assert_eq!(
        second.proof_frontier.unwrap().candidate_revision,
        identity.candidate_revision
    );
}

#[test]
fn proposal_hash_covers_ordered_boundary_and_numeric_contract() {
    let mut workspace = binary_workspace(true, "add");
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let revision = workspace
        .candidate_revision(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    let proposal = SpeculativeRewriteProposal {
        target: ImplOperationId::new("iop3"),
        replacement: binary_fragment("sub", "$left", "$right"),
        expected_before_impl_hash: identity.impl_hash,
        allow_speculative: true,
        claimed_rule: None,
    };
    let canonical = canonicalize_proposal_with_limit(
        &revision.impl_program,
        &proposal,
        &ResourceLimits::default(),
    )
    .unwrap();

    let mut reversed = proposal.clone();
    reversed.replacement.inputs.swap(0, 1);
    let reversed = canonicalize_proposal_with_limit(
        &revision.impl_program,
        &reversed,
        &ResourceLimits::default(),
    )
    .unwrap();
    assert_ne!(canonical.proposal_hash, reversed.proposal_hash);

    let mut changed_contract = revision.impl_program.clone();
    changed_contract.numeric_contract.reassociation = true;
    let changed =
        canonicalize_proposal_with_limit(&changed_contract, &proposal, &ResourceLimits::default())
            .unwrap();
    assert_ne!(canonical.proposal_hash, changed.proposal_hash);
}
