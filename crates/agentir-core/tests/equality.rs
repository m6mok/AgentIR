use agentir_core::{
    Action, CandidateId, CandidateRevisionId, ProposalId, RevisionId, Transaction, Workspace,
    WorkspaceId,
    candidate::{
        EQUALITY_CANDIDATE_CANONICAL_VERSION, ProposalInput, ProposalOperation, ProposalResult,
        ProposedImplFragment, RelationKind, SpeculativeRewriteProposal,
    },
    equality::{EqualityHash, EqualityStatus},
    ids::ImplOperationId,
    ir::ConstantValue,
};
use serde_json::json;
use std::collections::BTreeMap;

fn equality_workspace() -> Workspace {
    let workspace_id = WorkspaceId::new("stage-2c");
    let mut workspace = Workspace::new(workspace_id.clone()).unwrap();
    workspace
        .apply(&Transaction {
            workspace: workspace_id.clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateConstant {
                    bind: "$a".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(2),
                },
                Action::CreateConstant {
                    bind: "$b".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(3),
                },
                Action::CreateConstant {
                    bind: "$c".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(4),
                },
                Action::CreateConstant {
                    bind: "$d".to_owned(),
                    ty: "i32".parse().unwrap(),
                    value: json!(5),
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
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: workspace_id,
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
}

fn rooted_space() -> (Workspace, agentir_core::equality::EqualityQuery) {
    let mut workspace = equality_workspace();
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let root = workspace
        .equality_create(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    (workspace, root)
}

#[test]
fn creation_and_queries_preserve_candidate_state_and_root_identity() {
    let mut workspace = equality_workspace();
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let candidate_before = workspace.candidate_forest().clone();
    let root = workspace
        .equality_create(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    assert_eq!(workspace.candidate_forest(), &candidate_before);
    assert_eq!(root.root_impl_hash, identity.impl_hash);
    let root_node = workspace
        .equality_store()
        .revision(&root.equality_space, &root.equality_revision)
        .unwrap()
        .nodes
        .values()
        .next()
        .unwrap();
    assert_eq!(root_node.impl_hash, identity.impl_hash);

    let snapshot = workspace.snapshot();
    let first = workspace
        .equality_query(&root.equality_space, &root.equality_revision)
        .unwrap();
    let second = workspace
        .equality_query(&root.equality_space, &root.equality_revision)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(workspace.snapshot(), snapshot);

    let before_missing = workspace.snapshot();
    assert!(
        workspace
            .equality_create(&identity.candidate, &CandidateRevisionId::new("cr999"))
            .is_err()
    );
    assert_eq!(workspace.snapshot(), before_missing);
}

#[test]
fn sealed_exact_anchor_can_saturate_and_materialize_without_being_edited() {
    let mut workspace = equality_workspace();
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let sealed = workspace
        .candidate_seal(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    let anchor_head = sealed.candidate_revision.clone();
    let root = workspace
        .equality_create(&identity.candidate, &anchor_head)
        .unwrap();
    let saturated = workspace
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            100,
        )
        .unwrap();
    let target = workspace
        .equality_store()
        .revision(&saturated.equality_space, &saturated.equality_revision)
        .unwrap()
        .nodes
        .keys()
        .next_back()
        .unwrap()
        .clone();
    let materialized = workspace
        .equality_materialize(
            &saturated.equality_space,
            &saturated.equality_revision,
            &saturated.equality_hash,
            &target,
        )
        .unwrap();
    assert_ne!(materialized.candidate, identity.candidate);
    assert_eq!(
        workspace.candidate_query(&identity.candidate).unwrap().head,
        anchor_head
    );
}

#[test]
fn independent_rewrite_orders_merge_and_resumed_saturation_is_identical() {
    let (mut one_shot, root) = rooted_space();
    let saturated = one_shot
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            100,
        )
        .unwrap();
    assert_eq!(saturated.status, EqualityStatus::FixedPoint);
    assert_ne!(saturated.equality_hash, root.equality_hash);
    assert_eq!(saturated.node_count, 5);
    assert_eq!(saturated.edge_count, 5);
    assert!(saturated.merged_nodes >= 1);

    let (mut resumed, mut cursor) = rooted_space();
    loop {
        let step = resumed
            .equality_expand(
                &cursor.equality_space,
                &cursor.equality_revision,
                &cursor.equality_hash,
                1,
            )
            .unwrap();
        cursor = resumed
            .equality_query(&step.equality_space, &step.equality_revision)
            .unwrap();
        if step.status == EqualityStatus::FixedPoint {
            break;
        }
    }
    assert_eq!(cursor.equality_hash, saturated.equality_hash);
    let one_shot_revision = one_shot
        .equality_store()
        .revision(&saturated.equality_space, &saturated.equality_revision)
        .unwrap();
    let resumed_revision = resumed
        .equality_store()
        .revision(&cursor.equality_space, &cursor.equality_revision)
        .unwrap();
    assert_eq!(one_shot_revision.nodes, resumed_revision.nodes);
    assert_eq!(one_shot_revision.edges, resumed_revision.edges);
    assert_eq!(one_shot_revision.worklist, resumed_revision.worklist);

    let (mut different_policy, different_root) = rooted_space();
    let mut limits = different_policy.resource_limits().clone();
    limits.equality_nodes_per_space = 20_000;
    limits.equality_edges_per_space = 200_000;
    different_policy.set_resource_limits(limits);
    let different_saturated = different_policy
        .equality_saturate(
            &different_root.equality_space,
            &different_root.equality_revision,
            &different_root.equality_hash,
            100,
        )
        .unwrap();
    assert_eq!(different_saturated.equality_hash, saturated.equality_hash);

    let target = one_shot_revision.nodes.keys().next_back().unwrap().clone();
    let explanation = one_shot
        .equality_explain(
            &saturated.equality_space,
            &saturated.equality_revision,
            &target,
        )
        .unwrap();
    assert_eq!(explanation.edges.len(), 3);
    assert_eq!(
        explanation.edges[0].descriptor.target.operation_order_index,
        2
    );
}

#[test]
fn selected_node_materializes_atomically_and_v6_snapshot_replays() {
    let (mut workspace, root) = rooted_space();
    let saturated = workspace
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            100,
        )
        .unwrap();
    let target = workspace
        .equality_store()
        .revision(&saturated.equality_space, &saturated.equality_revision)
        .unwrap()
        .nodes
        .keys()
        .next_back()
        .unwrap()
        .clone();
    let anchor_head = workspace
        .candidate_query(&CandidateId::new("c1"))
        .unwrap()
        .head
        .clone();
    let materialized = workspace
        .equality_materialize(
            &saturated.equality_space,
            &saturated.equality_revision,
            &saturated.equality_hash,
            &target,
        )
        .unwrap();
    assert_eq!(
        workspace
            .candidate_query(&CandidateId::new("c1"))
            .unwrap()
            .head,
        anchor_head
    );
    let terminal = workspace
        .candidate_revision(&materialized.candidate, &materialized.candidate_revision)
        .unwrap();
    assert_eq!(
        terminal.impl_hash,
        workspace
            .equality_store()
            .revision(&saturated.equality_space, &saturated.equality_revision)
            .unwrap()
            .nodes[&target]
            .impl_hash
    );
    assert_eq!(
        terminal.candidate_hash_version,
        EQUALITY_CANDIDATE_CANONICAL_VERSION
    );
    assert_eq!(terminal.equality_materializations.len(), 1);

    let sealed = workspace
        .candidate_seal(&materialized.candidate, &materialized.candidate_revision)
        .unwrap();
    assert_eq!(
        sealed.state,
        agentir_core::candidate::CandidateState::Sealed
    );

    let snapshot = workspace.snapshot();
    let (replayed, report) = Workspace::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(report.equality_spaces_verified, 1);
    assert_eq!(replayed.snapshot(), snapshot);
}

#[test]
fn multi_edge_equality_path_discharges_matching_speculative_debt() {
    let mut workspace = equality_workspace();
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let root = workspace
        .equality_create(&identity.candidate, &identity.candidate_revision)
        .unwrap();
    let saturated = workspace
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            100,
        )
        .unwrap();
    let target_operands = workspace
        .candidate_revision(&identity.candidate, &identity.candidate_revision)
        .unwrap()
        .impl_program
        .operations[&ImplOperationId::new("iop7")]
        .operands
        .clone();
    let speculative = workspace
        .candidate_propose(
            &identity.candidate,
            &identity.candidate_revision,
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop7"),
                replacement: ProposedImplFragment {
                    inputs: vec![
                        ProposalInput {
                            bind: "$left".to_owned(),
                            value: target_operands[0].clone(),
                        },
                        ProposalInput {
                            bind: "$right".to_owned(),
                            value: target_operands[1].clone(),
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
    let target_hash = workspace
        .candidate_revision(&identity.candidate, &speculative.candidate_revision)
        .unwrap()
        .impl_hash
        .clone();
    let equality_revision = workspace
        .equality_store()
        .revision(&saturated.equality_space, &saturated.equality_revision)
        .unwrap();
    let target = equality_revision
        .nodes
        .values()
        .find(|node| node.impl_hash == target_hash)
        .unwrap()
        .id
        .clone();
    let explanation = workspace
        .equality_explain(
            &saturated.equality_space,
            &saturated.equality_revision,
            &target,
        )
        .unwrap();
    assert_eq!(explanation.edges.len(), 3);
    let discharged = workspace
        .candidate_equality_check(
            &identity.candidate,
            &speculative.candidate_revision,
            &ProposalId::new("p1"),
            &saturated.equality_space,
            &saturated.equality_revision,
            &saturated.equality_hash,
            &target,
        )
        .unwrap();
    let checked = workspace
        .candidate_check(&identity.candidate, &discharged.candidate_revision)
        .unwrap();
    assert!(checked.sealable);
    assert!(checked.open_obligations.is_empty());
    let revision = workspace
        .candidate_revision(&identity.candidate, &discharged.candidate_revision)
        .unwrap();
    assert_eq!(revision.equality_proofs.len(), 1);
    assert_eq!(
        revision.candidate_hash_version,
        EQUALITY_CANDIDATE_CANONICAL_VERSION
    );
}

#[test]
fn stale_hash_and_hard_limit_failures_leave_both_stores_unchanged() {
    let (mut workspace, root) = rooted_space();
    let before = workspace.snapshot();
    assert!(
        workspace
            .equality_expand(
                &root.equality_space,
                &root.equality_revision,
                &EqualityHash::new("stale"),
                1,
            )
            .is_err()
    );
    assert_eq!(workspace.snapshot(), before);

    let mut limits = workspace.resource_limits().clone();
    limits.equality_nodes_per_space = 1;
    workspace.set_resource_limits(limits);
    assert!(
        workspace
            .equality_expand(
                &root.equality_space,
                &root.equality_revision,
                &root.equality_hash,
                1,
            )
            .is_err()
    );
    let mut current = workspace.snapshot();
    current.schema_version = before.schema_version;
    assert_eq!(current, before);
    assert_eq!(
        workspace
            .candidate_query(&CandidateId::new("c1"))
            .unwrap()
            .head,
        CandidateRevisionId::new("cr1")
    );
}
