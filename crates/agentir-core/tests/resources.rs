use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionOpSpec, RegionSpec},
    canonical::canonical_bytes,
    resources::ResourceLimits,
};
use serde_json::json;
use std::collections::BTreeMap;

fn constant(bind: &str) -> Action {
    Action::CreateConstant {
        bind: bind.to_owned(),
        ty: "i32".parse().unwrap(),
        value: json!(1),
    }
}

#[test]
fn action_limit_boundary_is_atomic() {
    let limits = ResourceLimits {
        actions_per_transaction: 1,
        ..ResourceLimits::default()
    };
    let mut workspace = Workspace::with_limits(WorkspaceId::new("budget"), limits).unwrap();
    let accepted = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![constant("$one")],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("exact limit accepted");
    let before = workspace.snapshot();
    let error = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: accepted.revision,
            actions: vec![constant("$two"), constant("$three")],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect_err("limit + 1 rejected");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn projected_operation_limit_rejects_before_ids_are_consumed() {
    let limits = ResourceLimits {
        operations_per_program: 1,
        ..ResourceLimits::default()
    };
    let mut workspace = Workspace::with_limits(WorkspaceId::new("ops"), limits).unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![constant("$one"), constant("$two")],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect_err("projected graph is too large");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn canonical_size_limit_is_structured_and_atomic() {
    let mut workspace = Workspace::new(WorkspaceId::new("canonical-budget")).unwrap();
    let root_bytes = canonical_bytes(&workspace.revision(workspace.head()).unwrap().program)
        .unwrap()
        .len();
    let limits = ResourceLimits {
        canonical_output_bytes: u64::try_from(root_bytes).unwrap(),
        ..ResourceLimits::default()
    };
    workspace.set_resource_limits(limits);
    let before = workspace.snapshot();
    let error = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![constant("$one")],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect_err("larger canonical state rejected");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn oversized_region_is_rejected_without_partial_operations() {
    let limits = ResourceLimits {
        region_operations: 0,
        ..ResourceLimits::default()
    };
    let mut workspace = Workspace::with_limits(WorkspaceId::new("region-budget"), limits).unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![Action::CreateOp {
                bind: "$mapped".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["missing".to_owned()],
                attributes: BTreeMap::default(),
                region: Some(RegionSpec {
                    arguments: vec![RegionArgumentSpec {
                        name: "x".to_owned(),
                        ty: "f32".parse().unwrap(),
                    }],
                    captures: Vec::new(),
                    operations: vec![RegionOpSpec {
                        bind: "$x2".to_owned(),
                        opcode: "add".to_owned(),
                        operands: vec!["x".to_owned(), "x".to_owned()],
                        attributes: BTreeMap::default(),
                    }],
                    yield_value: "$x2".to_owned(),
                }),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect_err("region preflight wins before reference resolution");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn duplicate_current_constraints_do_not_consume_projected_budget() {
    let limits = ResourceLimits {
        constraints_per_program: 1,
        ..ResourceLimits::default()
    };
    let mut workspace =
        Workspace::with_limits(WorkspaceId::new("constraint-budget"), limits).unwrap();
    let equality = agentir_core::shapes::ShapeConstraint::Equal {
        left: "[N]".parse().unwrap(),
        right: "[4]".parse().unwrap(),
    };
    let commit = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::DefineDimension {
                    bind: None,
                    name: "N".to_owned(),
                    constraints: vec!["N >= 0".to_owned()],
                },
                Action::AddConstraint {
                    constraint: equality.clone(),
                },
                Action::AddConstraint {
                    constraint: equality,
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("fact-level duplicate stays within one retained constraint");
    assert_eq!(
        workspace
            .revision(&commit.revision)
            .unwrap()
            .program
            .constraints
            .len(),
        1
    );
}
