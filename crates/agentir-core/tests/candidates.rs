use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{
        CandidateAction, CandidateState, CandidateTransaction, DifferentialValidation,
        ELIMINATE_NOOP_CAST_RULE, FOLD_SCALAR_CONSTANTS_RULE, KNOWN_REWRITE_RULES,
        PRUNE_UNREACHABLE_RULE, RelationKind, known_rewrite_rule,
    },
    ids::{CandidateId, CandidateRevisionId, ImplOperationId, ValueId},
    impl_ir::{impl_hash, verify_impl},
    resources::ResourceLimits,
};
use serde_json::json;
use std::collections::BTreeMap;

fn constant_workspace(left: i32, right: i32, opcode: &str) -> Workspace {
    let id = WorkspaceId::new("candidate-test");
    let mut workspace = Workspace::new(id.clone()).expect("workspace");
    workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateConstant {
                    bind: "$left".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(left),
                },
                Action::CreateConstant {
                    bind: "$right".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(right),
                },
                Action::CreateOp {
                    bind: "$result".to_owned(),
                    opcode: opcode.to_owned(),
                    operands: vec!["$left".to_owned(), "$right".to_owned()],
                    attributes: BTreeMap::new(),
                    region: None,
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$result".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("build");
    workspace
}

fn freeze(workspace: &mut Workspace) -> RevisionId {
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("freeze")
        .revision
}

fn cast_workspace(target: &str) -> Workspace {
    let id = WorkspaceId::new("cast-candidate");
    let mut workspace = Workspace::new(id.clone()).unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().unwrap(),
                },
                Action::CreateOp {
                    bind: "$cast".to_owned(),
                    opcode: "cast".to_owned(),
                    operands: vec!["$x".to_owned()],
                    attributes: BTreeMap::from([("target_type".to_owned(), json!(target))]),
                    region: None,
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$cast".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
}

#[test]
fn candidate_requires_frozen_spec_and_identity_is_separate_and_proved() {
    assert_eq!(
        KNOWN_REWRITE_RULES
            .iter()
            .map(|rule| rule.id)
            .collect::<Vec<_>>(),
        [
            ELIMINATE_NOOP_CAST_RULE,
            FOLD_SCALAR_CONSTANTS_RULE,
            PRUNE_UNREACHABLE_RULE,
        ]
    );
    assert!(known_rewrite_rule("invented").is_none());

    let mut workspace = constant_workspace(2, 3, "add");
    let before = workspace.snapshot();
    let error = workspace
        .candidate_create(&RevisionId::new("r1"), RelationKind::EquivalentToSpec)
        .expect_err("draft SpecIR is rejected");
    assert_eq!(error.code, ErrorCode::SpecNotFrozen);
    assert_eq!(workspace.snapshot(), before);

    let frozen = freeze(&mut workspace);
    let report = workspace
        .candidate_create(&frozen, RelationKind::EquivalentToSpec)
        .expect("identity candidate");
    assert_eq!(report.candidate, CandidateId::new("c1"));
    assert_eq!(report.candidate_revision, CandidateRevisionId::new("cr1"));
    assert!(report.well_typed && report.sealable);
    assert_eq!(report.state, CandidateState::Equivalent);
    let candidate = workspace.candidate_query(&report.candidate).unwrap();
    assert_eq!(candidate.spec_revision, frozen);
    assert_eq!(candidate.spec_hash, report.equivalence.spec_hash);
    assert_ne!(
        candidate
            .revisions
            .get(&report.candidate_revision)
            .unwrap()
            .impl_program
            .operations
            .keys()
            .next()
            .unwrap()
            .as_str(),
        "op1"
    );

    workspace.set_resource_limits(ResourceLimits {
        candidates_per_workspace: 1,
        ..ResourceLimits::default()
    });
    let before_limit = workspace.snapshot();
    let error = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .expect_err("candidate count is checked before ID allocation");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before_limit);
    workspace.set_resource_limits(ResourceLimits::default());
    let second = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .expect("independent candidate for the same frozen spec");
    assert_eq!(second.candidate, CandidateId::new("c2"));
    assert_eq!(second.impl_hash, report.impl_hash);
    assert_ne!(second.candidate_hash, report.candidate_hash);

    let unsupported = workspace
        .candidate_create(
            &RevisionId::new("r2"),
            RelationKind::RefinesSpecWithinTolerance,
        )
        .expect_err("approximate refinement is outside Stage 2A");
    assert_eq!(unsupported.code, ErrorCode::UnsupportedRefinement);
}

#[test]
fn exact_rewrite_chain_is_atomic_and_distinguishes_impl_and_candidate_hashes() {
    let mut workspace = constant_workspace(2, 3, "add");
    freeze(&mut workspace);
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let continuation = workspace
        .candidate_continuation(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    assert_eq!(continuation.matches.len(), 1);
    assert_eq!(continuation.matches[0].rule, FOLD_SCALAR_CONSTANTS_RULE);
    assert_eq!(continuation.matches[0].target, ImplOperationId::new("iop3"));

    let before_rejection = workspace.snapshot();
    let stale = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate.clone(),
            base_revision: identity.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: Some(agentir_core::impl_ir::ImplHash::new("stale")),
            }],
        })
        .expect_err("stale hash precondition");
    assert_eq!(stale.code, ErrorCode::RewritePreconditionFailed);
    assert_eq!(workspace.snapshot(), before_rejection);

    let folded = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate.clone(),
            base_revision: identity.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: Some(identity.impl_hash),
            }],
        })
        .unwrap();
    assert_eq!(folded.correctness_evidence, 2);

    let pruned = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: folded.candidate.clone(),
            base_revision: folded.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: PRUNE_UNREACHABLE_RULE.to_owned(),
                target: ImplOperationId::new("iop1"),
                expected_before_impl_hash: Some(folded.impl_hash.clone()),
            }],
        })
        .unwrap();
    assert_eq!(folded.impl_hash, pruned.impl_hash);
    assert_ne!(folded.candidate_hash, pruned.candidate_hash);
    assert_eq!(pruned.correctness_evidence, 3);

    let stale_base = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: folded.candidate.clone(),
            base_revision: folded.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: PRUNE_UNREACHABLE_RULE.to_owned(),
                target: ImplOperationId::new("iop2"),
                expected_before_impl_hash: None,
            }],
        })
        .expect_err("stale candidate base");
    assert_eq!(stale_base.code, ErrorCode::CandidateRevisionNotFound);

    let sealed = workspace
        .candidate_seal(&pruned.candidate, &pruned.candidate_revision)
        .unwrap();
    assert_eq!(sealed.state, CandidateState::Sealed);
    let sealed_again = workspace
        .candidate_seal(&sealed.candidate, &sealed.candidate_revision)
        .unwrap();
    assert_eq!(sealed_again.candidate_revision, sealed.candidate_revision);
    let before_sealed_edit = workspace.snapshot();
    let error = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: sealed.candidate.clone(),
            base_revision: sealed.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: "invented".to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: None,
            }],
        })
        .expect_err("sealed candidates are immutable before rule matching");
    assert_eq!(error.code, ErrorCode::CandidateSealed);
    assert_eq!(workspace.snapshot(), before_sealed_edit);
    let error = workspace
        .candidate_record_validation(
            &sealed.candidate,
            &sealed.candidate_revision,
            DifferentialValidation {
                seed: 1,
                requested_cases: 1,
                executed_cases: 1,
                passed: true,
                counterexample: None,
            },
        )
        .expect_err("sealed candidates cannot accumulate new evidence revisions");
    assert_eq!(error.code, ErrorCode::CandidateSealed);
    assert_eq!(workspace.snapshot(), before_sealed_edit);
    let fork = workspace
        .candidate_fork(&sealed.candidate, &sealed.candidate_revision)
        .unwrap();
    assert_eq!(fork.candidate, CandidateId::new("c2"));
    assert_eq!(fork.state, CandidateState::Draft);
    assert_eq!(fork.impl_hash, sealed.impl_hash);
    assert_ne!(fork.candidate_hash, sealed.candidate_hash);
}

#[test]
fn impl_hash_tracks_semantics_not_ids_evidence_or_unreachable_state() {
    let mut workspace = constant_workspace(2, 3, "sub");
    freeze(&mut workspace);
    let first = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let second = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    assert_eq!(first.impl_hash, second.impl_hash);
    assert_ne!(first.candidate_hash, second.candidate_hash);

    let original = workspace
        .candidate_revision(&first.candidate, &first.candidate_revision)
        .unwrap()
        .impl_program
        .clone();
    let mut reordered = original.clone();
    reordered
        .operations
        .get_mut(&ImplOperationId::new("iop3"))
        .unwrap()
        .operands
        .reverse();
    assert_ne!(
        impl_hash(&original).unwrap(),
        impl_hash(&reordered).unwrap()
    );

    let mut numeric = original.clone();
    numeric.numeric_contract.reassociation = true;
    assert_ne!(impl_hash(&original).unwrap(), impl_hash(&numeric).unwrap());

    let mut corrupted_source = original;
    corrupted_source
        .values
        .get_mut(&agentir_core::ids::ImplValueId::new("iv1"))
        .unwrap()
        .source_link
        .spec_value = Some(ValueId::new("v999"));
    let error = verify_impl(
        &corrupted_source,
        &workspace.revision(&RevisionId::new("r2")).unwrap().program,
        &ResourceLimits::default(),
    )
    .expect_err("corrupted source links fail the ImplIR verifier");
    assert_eq!(error.code, ErrorCode::ImplVerificationFailed);
}

#[test]
fn undefined_constant_folds_are_rejected_without_consuming_candidate_ids() {
    for (left, right, opcode) in [(i32::MAX, 1, "add"), (7, 0, "div")] {
        let mut workspace = constant_workspace(left, right, opcode);
        freeze(&mut workspace);
        let identity = workspace
            .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
            .unwrap();
        assert!(
            workspace
                .candidate_continuation(&identity.candidate, &identity.candidate_revision)
                .unwrap()
                .matches
                .is_empty()
        );
        let before = workspace.snapshot();
        let error = workspace
            .candidate_apply(&CandidateTransaction {
                candidate: identity.candidate,
                base_revision: identity.candidate_revision,
                actions: vec![CandidateAction::ApplyKnownRewrite {
                    rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                    target: ImplOperationId::new("iop3"),
                    expected_before_impl_hash: None,
                }],
            })
            .expect_err("undefined fold rejected");
        assert_eq!(error.code, ErrorCode::RewritePreconditionFailed);
        assert_eq!(workspace.snapshot(), before);
    }
}

#[test]
fn candidate_resource_rejections_are_atomic_and_count_the_pending_step() {
    let mut workspace = constant_workspace(2, 3, "add");
    freeze(&mut workspace);
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();

    workspace.set_resource_limits(ResourceLimits {
        candidate_actions_per_transaction: 0,
        ..ResourceLimits::default()
    });
    let before_action_limit = workspace.snapshot();
    let error = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate.clone(),
            base_revision: identity.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: None,
            }],
        })
        .expect_err("candidate action count is checked before graph staging");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before_action_limit);

    let limits = ResourceLimits {
        rewrite_steps_per_candidate: 1,
        ..ResourceLimits::default()
    };
    workspace.set_resource_limits(limits);
    let folded = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate.clone(),
            base_revision: identity.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: None,
            }],
        })
        .expect("the first rewrite is within a one-step budget");
    let before = workspace.snapshot();
    let error = workspace
        .candidate_apply(&CandidateTransaction {
            candidate: folded.candidate,
            base_revision: folded.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: PRUNE_UNREACHABLE_RULE.to_owned(),
                target: ImplOperationId::new("iop1"),
                expected_before_impl_hash: None,
            }],
        })
        .expect_err("a second rewrite exceeds the one-step budget");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);

    let mut match_limits = workspace.resource_limits().clone();
    match_limits.rewrite_matches_per_continuation = 0;
    workspace.set_resource_limits(match_limits);
    let first = workspace
        .candidate_continuation(&identity.candidate, &identity.candidate_revision)
        .expect_err("the first discovered match exceeds a zero-match budget");
    let second = workspace
        .candidate_continuation(&identity.candidate, &identity.candidate_revision)
        .expect_err("the limit result is deterministic");
    assert_eq!(first.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(first, second);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn evidence_size_rejection_is_structured_atomic_and_does_not_consume_ids() {
    let mut workspace = constant_workspace(2, 3, "add");
    freeze(&mut workspace);
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace.set_resource_limits(ResourceLimits {
        evidence_bytes: 0,
        ..ResourceLimits::default()
    });
    let before = workspace.snapshot();
    let error = workspace
        .candidate_record_validation(
            &identity.candidate,
            &identity.candidate_revision,
            DifferentialValidation {
                seed: 7,
                requested_cases: 1,
                executed_cases: 1,
                passed: true,
                counterexample: None,
            },
        )
        .expect_err("projected EvidenceIR bytes exceed the limit");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);

    workspace.set_resource_limits(ResourceLimits::default());
    let before_invalid_evidence = workspace.snapshot();
    let error = workspace
        .candidate_record_validation(
            &identity.candidate,
            &identity.candidate_revision,
            DifferentialValidation {
                seed: 7,
                requested_cases: 1,
                executed_cases: 1,
                passed: false,
                counterexample: None,
            },
        )
        .expect_err("failed confidence evidence requires a counterexample");
    assert_eq!(error.code, ErrorCode::EvidenceInvalid);
    assert_eq!(workspace.snapshot(), before_invalid_evidence);

    let accepted = workspace
        .candidate_record_validation(
            &identity.candidate,
            &identity.candidate_revision,
            DifferentialValidation {
                seed: 7,
                requested_cases: 1,
                executed_cases: 1,
                passed: true,
                counterexample: None,
            },
        )
        .expect("the same event is accepted after restoring the limit");
    assert_eq!(accepted.candidate_revision, CandidateRevisionId::new("cr2"));
}

#[test]
fn noop_cast_elimination_requires_fully_identical_types() {
    let mut exact = cast_workspace("f32");
    freeze(&mut exact);
    let identity = exact
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let continuation = exact
        .candidate_continuation(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    assert!(continuation.matches.iter().any(|entry| {
        entry.rule == ELIMINATE_NOOP_CAST_RULE && entry.target == ImplOperationId::new("iop2")
    }));
    let eliminated = exact
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate,
            base_revision: identity.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: ELIMINATE_NOOP_CAST_RULE.to_owned(),
                target: ImplOperationId::new("iop2"),
                expected_before_impl_hash: None,
            }],
        })
        .unwrap();
    assert_eq!(
        exact
            .candidate_revision(&eliminated.candidate, &eliminated.candidate_revision)
            .unwrap()
            .impl_program
            .operations
            .len(),
        1
    );

    let mut changing = cast_workspace("i32");
    freeze(&mut changing);
    let identity = changing
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let error = changing
        .candidate_apply(&CandidateTransaction {
            candidate: identity.candidate,
            base_revision: identity.candidate_revision,
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: ELIMINATE_NOOP_CAST_RULE.to_owned(),
                target: ImplOperationId::new("iop2"),
                expected_before_impl_hash: None,
            }],
        })
        .expect_err("value-changing cast is not a no-op");
    assert_eq!(error.code, ErrorCode::RewriteNotApplicable);
}

#[test]
fn candidate_snapshot_replays_and_rejects_hash_chain_and_semantics_corruption() {
    let mut workspace = constant_workspace(2, 3, "add");
    freeze(&mut workspace);
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
    let snapshot = workspace.snapshot();
    let (restored, report) = Workspace::from_snapshot(snapshot.clone()).expect("replay");
    assert_eq!(restored.snapshot(), snapshot);
    assert_eq!(report.candidates_verified, 1);
    assert_eq!(report.candidate_events_replayed, 3);

    let mut bad_hash = snapshot.clone();
    bad_hash
        .candidate_forest
        .candidates
        .get_mut(&CandidateId::new("c1"))
        .unwrap()
        .revisions
        .get_mut(&CandidateRevisionId::new("cr3"))
        .unwrap()
        .candidate_hash = agentir_core::candidate::CandidateHash::new("corrupted");
    assert_eq!(
        Workspace::from_snapshot(bad_hash).unwrap_err().code,
        ErrorCode::ReplayMismatch
    );

    let mut bad_semantics = snapshot;
    bad_semantics.candidate_forest.events[0].semantics_version = 99;
    assert_eq!(
        Workspace::from_snapshot(bad_semantics).unwrap_err().code,
        ErrorCode::PersistenceFormat
    );
}
