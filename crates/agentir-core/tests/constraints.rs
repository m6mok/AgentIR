use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    constraints::{ConstraintFacts, ConstraintQueryResult},
    persistence::{CORE_SEMANTICS_VERSION, LEGACY_CORE_SEMANTICS_VERSION},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    shapes::ShapeConstraint,
    types::{DimExpr, Shape},
};
use std::collections::BTreeMap;

fn equal(left: &str, right: &str) -> ShapeConstraint {
    ShapeConstraint::Equal {
        left: left.parse().expect("left shape"),
        right: right.parse().expect("right shape"),
    }
}

fn facts(symbols: &[&str]) -> ConstraintFacts {
    let mut facts = ConstraintFacts::default();
    for symbol in symbols {
        facts
            .declare_symbol(symbol, true)
            .expect("symbol declaration");
    }
    facts
}

fn status(result: &ConstraintQueryResult) -> &'static str {
    match result {
        ConstraintQueryResult::Proved { .. } => "proved",
        ConstraintQueryResult::Contradiction { .. } => "contradiction",
        ConstraintQueryResult::Unknown => "unknown",
    }
}

#[test]
fn compact_facts_prove_transitive_and_static_equalities() {
    let mut facts = facts(&["N", "M", "K"]);
    facts.insert(&equal("[N]", "[M]")).expect("N == M");
    facts.insert(&equal("[M]", "[K]")).expect("M == K");
    assert_eq!(
        status(
            &facts
                .query_shapes(&"[N]".parse().unwrap(), &"[K]".parse().unwrap())
                .unwrap()
        ),
        "proved"
    );
    facts.insert(&equal("[K]", "[4]")).expect("K == 4");
    assert_eq!(
        status(
            &facts
                .query_shapes(&"[N]".parse().unwrap(), &"[4]".parse().unwrap())
                .unwrap()
        ),
        "proved"
    );
}

#[test]
fn contradictions_unknowns_reordering_and_duplicates_are_deterministic() {
    let mut model = facts(&["N", "M"]);
    model.insert(&equal("[N]", "[4]")).expect("N == 4");
    let before = model.accepted_facts();
    model.insert(&equal("[N]", "[4]")).expect("duplicate");
    assert_eq!(model.accepted_facts(), before);
    let error = model
        .insert(&equal("[N]", "[5]"))
        .expect_err("N cannot be both 4 and 5");
    assert_eq!(error.code, ErrorCode::ConstraintContradiction);
    assert_eq!(model.accepted_facts(), before);

    let mut affine = facts(&["N", "M"]);
    let affine_equality = equal("[2*N+1]", "[3*M]");
    assert_eq!(
        status(
            &affine
                .query_shapes(&"[2*N+1]".parse().unwrap(), &"[3*M]".parse().unwrap())
                .unwrap()
        ),
        "unknown"
    );
    affine
        .insert(&affine_equality)
        .expect("direct affine fact is sufficient evidence");
    assert_eq!(
        status(
            &affine
                .query_shapes(&"[2*N+1]".parse().unwrap(), &"[3*M]".parse().unwrap())
                .unwrap()
        ),
        "proved"
    );

    let constraints = [equal("[N]", "[M]"), equal("[M]", "[4]")];
    let mut forward = facts(&["N", "M"]);
    let mut reverse = facts(&["N", "M"]);
    for constraint in &constraints {
        forward.insert(constraint).unwrap();
    }
    for constraint in constraints.iter().rev() {
        reverse.insert(constraint).unwrap();
    }
    let query_left: Shape = "[N]".parse().unwrap();
    let query_right: Shape = "[4]".parse().unwrap();
    assert_eq!(
        status(&forward.query_shapes(&query_left, &query_right).unwrap()),
        status(&reverse.query_shapes(&query_left, &query_right).unwrap())
    );
}

#[test]
fn alpha_renaming_preserves_proof_classification() {
    let mut first = facts(&["N", "M"]);
    first.insert(&equal("[N]", "[M]")).unwrap();
    let mut second = facts(&["X", "Y"]);
    second.insert(&equal("[X]", "[Y]")).unwrap();
    assert_eq!(
        status(
            &first
                .query_shapes(&"[N]".parse().unwrap(), &"[M]".parse().unwrap())
                .unwrap()
        ),
        status(
            &second
                .query_shapes(&"[X]".parse().unwrap(), &"[Y]".parse().unwrap())
                .unwrap()
        )
    );
}

fn build_actions(constraint_before_operation: bool) -> Vec<Action> {
    let mut actions = vec![
        Action::DefineDimension {
            bind: None,
            name: "N".to_owned(),
            constraints: vec!["N >= 0".to_owned()],
        },
        Action::DefineDimension {
            bind: None,
            name: "M".to_owned(),
            constraints: vec!["M >= 0".to_owned()],
        },
        Action::DefineDimension {
            bind: None,
            name: "K".to_owned(),
            constraints: vec!["K >= 0".to_owned()],
        },
        Action::CreateParameter {
            bind: "$x".to_owned(),
            name: "x".to_owned(),
            ty: "tensor<f32,[N]>".parse().unwrap(),
        },
        Action::CreateParameter {
            bind: "$y".to_owned(),
            name: "y".to_owned(),
            ty: "tensor<f32,[M]>".parse().unwrap(),
        },
    ];
    if constraint_before_operation {
        actions.push(Action::AddConstraint {
            constraint: equal("[N]", "[M]"),
        });
    }
    actions.extend([
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
    ]);
    actions
}

#[test]
fn constraints_discharge_only_relevant_shape_obligations() {
    let mut workspace = Workspace::new(WorkspaceId::new("discharge")).unwrap();
    let built = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: build_actions(false),
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert_eq!(
        workspace
            .check(&built.revision)
            .unwrap()
            .open_obligations
            .len(),
        1
    );
    let unrelated = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: built.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[N]", "[K]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert_eq!(
        workspace
            .check(&unrelated.revision)
            .unwrap()
            .open_obligations
            .len(),
        1
    );
    let discharged = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: unrelated.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[M]", "[K]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert!(workspace.check(&discharged.revision).unwrap().complete);
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: discharged.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("freeze after proof discharge");
}

#[test]
fn a_prior_constraint_prevents_shape_obligation_creation() {
    let mut workspace = Workspace::new(WorkspaceId::new("prior")).unwrap();
    let built = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: build_actions(true),
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let revision = workspace.revision(&built.revision).unwrap();
    assert!(revision.program.obligations.values().all(|obligation| {
        obligation.kind != agentir_core::obligations::ObligationKind::ShapeCompatible
    }));
    assert!(workspace.check(&built.revision).unwrap().complete);
}

#[test]
fn symbol_to_static_fact_proves_operation_compatibility_immediately() {
    let mut workspace = Workspace::new(WorkspaceId::new("static-prior")).unwrap();
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
                    constraint: equal("[N]", "[4]"),
                },
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "tensor<f32,[N]>".parse().unwrap(),
                },
                Action::CreateParameter {
                    bind: "$y".to_owned(),
                    name: "y".to_owned(),
                    ty: "tensor<f32,[4]>".parse().unwrap(),
                },
                Action::CreateOp {
                    bind: "$sum".to_owned(),
                    opcode: "add".to_owned(),
                    operands: vec!["$x".to_owned(), "$y".to_owned()],
                    attributes: BTreeMap::new(),
                    region: None,
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert!(
        workspace
            .revision(&commit.revision)
            .unwrap()
            .program
            .obligations
            .values()
            .all(|obligation| obligation.kind
                != agentir_core::obligations::ObligationKind::ShapeCompatible)
    );
}

#[test]
fn typed_hole_shape_obligation_discharges_from_later_fact() {
    let mut workspace = Workspace::new(WorkspaceId::new("hole-discharge")).unwrap();
    let built = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::DefineDimension {
                    bind: None,
                    name: "N".to_owned(),
                    constraints: vec!["N >= 0".to_owned()],
                },
                Action::DefineDimension {
                    bind: None,
                    name: "M".to_owned(),
                    constraints: vec!["M >= 0".to_owned()],
                },
                Action::CreateParameter {
                    bind: "$value".to_owned(),
                    name: "value".to_owned(),
                    ty: "tensor<f32,[M]>".parse().unwrap(),
                },
                Action::CreateHole {
                    bind: "$hole".to_owned(),
                    expected_type: "tensor<f32,[N]>".parse().unwrap(),
                    shape_constraints: Vec::new(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$hole".to_owned(),
                },
                Action::FillHole {
                    hole: "$hole".to_owned(),
                    value: "$value".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert_eq!(
        workspace
            .check(&built.revision)
            .unwrap()
            .open_obligations
            .len(),
        1
    );
    let discharged = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: built.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[N]", "[M]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert!(workspace.check(&discharged.revision).unwrap().complete);
}

#[test]
fn direct_affine_fact_discharges_the_exact_shape_relation() {
    let mut workspace = Workspace::new(WorkspaceId::new("affine-discharge")).unwrap();
    let built = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::DefineDimension {
                    bind: None,
                    name: "N".to_owned(),
                    constraints: vec!["N >= 0".to_owned()],
                },
                Action::DefineDimension {
                    bind: None,
                    name: "M".to_owned(),
                    constraints: vec!["M >= 0".to_owned()],
                },
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "tensor<f32,[2*N+1]>".parse().unwrap(),
                },
                Action::CreateParameter {
                    bind: "$y".to_owned(),
                    name: "y".to_owned(),
                    ty: "tensor<f32,[3*M]>".parse().unwrap(),
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
    assert_eq!(
        workspace
            .check(&built.revision)
            .unwrap()
            .open_obligations
            .len(),
        1
    );
    let discharged = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: built.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[2*N+1]", "[3*M]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    assert!(workspace.check(&discharged.revision).unwrap().complete);
}

#[test]
fn rejected_constraint_is_atomic_and_does_not_consume_ids() {
    let mut workspace = Workspace::new(WorkspaceId::new("atomic-constraint")).unwrap();
    let declared = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![Action::DefineDimension {
                bind: None,
                name: "N".to_owned(),
                constraints: vec!["N >= 0".to_owned()],
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let first = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: declared.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[N]", "[4]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: first.revision,
            actions: vec![Action::AddConstraint {
                constraint: equal("[N]", "[5]"),
            }],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect_err("contradiction");
    assert_eq!(error.code, ErrorCode::ConstraintContradiction);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn different_construction_histories_share_spec_hash_after_freeze() {
    let mut hashes = Vec::new();
    for (index, prior) in [false, true].into_iter().enumerate() {
        let mut workspace = Workspace::new(WorkspaceId::new(format!("history-{index}"))).unwrap();
        let built = workspace
            .apply(&Transaction {
                workspace: workspace.id().clone(),
                base_revision: RevisionId::new("r0"),
                actions: build_actions(prior),
                client_transaction_id: None,
                allow_branch: false,
            })
            .unwrap();
        let ready = if prior {
            built.revision
        } else {
            workspace
                .apply(&Transaction {
                    workspace: workspace.id().clone(),
                    base_revision: built.revision,
                    actions: vec![Action::AddConstraint {
                        constraint: equal("[N]", "[M]"),
                    }],
                    client_transaction_id: None,
                    allow_branch: false,
                })
                .unwrap()
                .revision
        };
        let frozen = workspace
            .apply(&Transaction {
                workspace: workspace.id().clone(),
                base_revision: ready,
                actions: vec![Action::FreezeSpec],
                client_transaction_id: None,
                allow_branch: false,
            })
            .unwrap();
        hashes.push((frozen.content_hash, frozen.spec_hash.unwrap()));
    }
    assert_ne!(hashes[0].0, hashes[1].0);
    assert_eq!(hashes[0].1, hashes[1].1);
}

fn evaluate_dimension(expression: &DimExpr, assignment: &BTreeMap<String, u64>) -> i128 {
    match expression {
        DimExpr::Static(value) => i128::from(*value),
        DimExpr::Symbol(symbol) => i128::from(assignment[symbol]),
        DimExpr::Affine {
            coefficient,
            symbol,
            constant,
        } => i128::from(*coefficient) * i128::from(assignment[symbol]) + i128::from(*constant),
    }
}

fn constraint_holds(constraint: &ShapeConstraint, assignment: &BTreeMap<String, u64>) -> bool {
    match constraint {
        ShapeConstraint::Equal { left, right } => {
            left.0.len() == right.0.len()
                && left.0.iter().zip(&right.0).all(|(left, right)| {
                    evaluate_dimension(left, assignment) == evaluate_dimension(right, assignment)
                })
        }
        ShapeConstraint::NonNegative { symbol } => assignment[symbol] <= 4,
    }
}

fn soundness_sequence(seed: u64) -> Vec<String> {
    BudgetCheck::against(
        &ResourceLimits::default(),
        ResourceKind::GeneratedCaseSize,
        48,
        "constraint soundness property corpus",
    )
    .unwrap();
    let mut state = seed;
    let mut outcomes = Vec::new();
    for _case in 0..48 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let static_value = state % 5;
        let mut facts = facts(&["N", "M", "K"]);
        let (constraints, left, right) = match state % 6 {
            0 => (
                vec![
                    equal("[N]", "[M]"),
                    equal("[M]", &format!("[{static_value}]")),
                ],
                "[2*N+1]".parse().unwrap(),
                format!("[{}]", 2 * static_value + 1).parse().unwrap(),
            ),
            1 => (
                vec![equal("[M]", "[K]"), equal("[N]", "[M]")],
                "[2*N+1]".parse().unwrap(),
                "[2*K+1]".parse().unwrap(),
            ),
            2 => (
                vec![equal("[N]", &format!("[{static_value}]"))],
                "[N]".parse().unwrap(),
                format!("[{}]", static_value + 1).parse().unwrap(),
            ),
            3 => (
                vec![equal("[N]", "[M]")],
                "[2*N+1]".parse().unwrap(),
                "[3*K]".parse().unwrap(),
            ),
            4 => (
                vec![equal("[2*N+1]", "[3*M]")],
                "[3*M]".parse().unwrap(),
                "[2*N+1]".parse().unwrap(),
            ),
            _ => (
                vec![equal("[N]", "[M]"), equal("[N]", "[M]")],
                "[N]".parse().unwrap(),
                "[M]".parse().unwrap(),
            ),
        };
        for constraint in &constraints {
            facts.insert(constraint).unwrap();
        }
        let result = facts.query_shapes(&left, &right).unwrap();
        let query = ShapeConstraint::Equal {
            left: left.clone(),
            right: right.clone(),
        };
        let mut satisfying_assignments = 0;
        let mut satisfying_query = 0;
        for n in 0..=4 {
            for m in 0..=4 {
                for k in 0..=4 {
                    let assignment = BTreeMap::from([
                        ("N".to_owned(), n),
                        ("M".to_owned(), m),
                        ("K".to_owned(), k),
                    ]);
                    if constraints
                        .iter()
                        .all(|constraint| constraint_holds(constraint, &assignment))
                    {
                        satisfying_assignments += 1;
                        if constraint_holds(&query, &assignment) {
                            satisfying_query += 1;
                        }
                    }
                }
            }
        }
        assert!(
            satisfying_assignments > 0,
            "oracle domain must be non-vacuous"
        );
        match &result {
            ConstraintQueryResult::Proved { .. } => {
                assert_eq!(satisfying_query, satisfying_assignments);
            }
            ConstraintQueryResult::Contradiction { contradiction } => {
                assert_eq!(satisfying_query, 0);
                assert!(!contradiction.conflicting_facts.is_empty());
                let before = facts.accepted_facts();
                let error = facts
                    .insert(&query)
                    .expect_err("contradictory fact rejected");
                assert_eq!(error.code, ErrorCode::ConstraintContradiction);
                assert_eq!(facts.accepted_facts(), before);
            }
            ConstraintQueryResult::Unknown => {}
        }
        outcomes.push(status(&result).to_owned());
    }
    outcomes
}

#[test]
fn fixed_seed_solver_soundness_is_reproducible() {
    assert_eq!(soundness_sequence(0x5eed), soundness_sequence(0x5eed));
}

#[test]
fn semantics_version_constants_are_distinct() {
    assert_eq!(LEGACY_CORE_SEMANTICS_VERSION, 1);
    assert_eq!(CORE_SEMANTICS_VERSION, 2);
}
