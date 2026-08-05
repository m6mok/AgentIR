use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    candidate::{
        CandidateAction, CandidateCheckReport, CandidateTransaction, ELIMINATE_NOOP_CAST_RULE,
        FOLD_SCALAR_CONSTANTS_RULE, PRUNE_UNREACHABLE_RULE, RelationKind,
    },
    resources::ResourceLimits,
};
use agentir_eval::differential_validate;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn freeze(workspace: &mut Workspace) {
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r1"),
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("freeze");
}

fn constant_candidate(
    label: &str,
    opcode: &str,
    constants: &[(&str, Value)],
    attributes: BTreeMap<String, Value>,
) -> (Workspace, CandidateCheckReport) {
    let id = WorkspaceId::new(label);
    let mut workspace = Workspace::new(id.clone()).expect("workspace");
    let mut actions = constants
        .iter()
        .enumerate()
        .map(|(index, (ty, value))| Action::CreateConstant {
            bind: format!("$c{index}"),
            ty: ty.parse().expect("constant type"),
            value: value.clone(),
        })
        .collect::<Vec<_>>();
    actions.push(Action::CreateOp {
        bind: "$result".to_owned(),
        opcode: opcode.to_owned(),
        operands: (0..constants.len())
            .map(|index| format!("$c{index}"))
            .collect(),
        attributes,
        region: None,
    });
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: "$result".to_owned(),
    });
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("constant graph");
    freeze(&mut workspace);
    let candidate = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .expect("identity candidate");
    (workspace, candidate)
}

fn apply_first_match(
    workspace: &mut Workspace,
    base: &CandidateCheckReport,
    rule: &str,
) -> CandidateCheckReport {
    let first = workspace
        .candidate_continuation(&base.candidate, &base.candidate_revision)
        .expect("continuation");
    let second = workspace
        .candidate_continuation(&base.candidate, &base.candidate_revision)
        .expect("repeated continuation");
    assert_eq!(first, second);
    let rewrite = first
        .matches
        .iter()
        .find(|entry| entry.rule == rule)
        .expect("production matcher found the rule");
    workspace
        .candidate_apply(&CandidateTransaction {
            candidate: base.candidate.clone(),
            base_revision: base.candidate_revision.clone(),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: rule.to_owned(),
                target: rewrite.target.clone(),
                expected_before_impl_hash: Some(first.expected_before_impl_hash),
            }],
        })
        .expect("production rewrite")
}

fn validate_current(workspace: &Workspace, candidate: &CandidateCheckReport, seed: u64) {
    let source = &workspace
        .revision(&RevisionId::new("r2"))
        .expect("frozen source")
        .program;
    let implementation = &workspace
        .candidate_revision(&candidate.candidate, &candidate.candidate_revision)
        .expect("candidate revision")
        .impl_program;
    let validation =
        differential_validate(source, implementation, seed, 16, &ResourceLimits::default())
            .expect("bounded oracle");
    assert!(validation.passed, "{validation:?}");
}

fn fixed_seed_soundness_sequence(seed: u64) -> Vec<String> {
    let cases = vec![
        (
            "add-i32",
            "add",
            vec![("i32", json!(2)), ("i32", json!(3))],
            BTreeMap::new(),
        ),
        (
            "sub-i32",
            "sub",
            vec![("i32", json!(-2)), ("i32", json!(3))],
            BTreeMap::new(),
        ),
        (
            "mul-i32",
            "mul",
            vec![("i32", json!(-2)), ("i32", json!(3))],
            BTreeMap::new(),
        ),
        (
            "div-i32",
            "div",
            vec![("i32", json!(-7)), ("i32", json!(3))],
            BTreeMap::new(),
        ),
        (
            "fma-i32",
            "fma",
            vec![("i32", json!(2)), ("i32", json!(3)), ("i32", json!(-1))],
            BTreeMap::new(),
        ),
        (
            "add-f32",
            "add",
            vec![("f32", json!(-0.0_f32)), ("f32", json!(-0.0_f32))],
            BTreeMap::new(),
        ),
        (
            "fma-f32",
            "fma",
            vec![
                ("f32", json!(0.5_f32)),
                ("f32", json!(2.0_f32)),
                ("f32", json!(-1.0_f32)),
            ],
            BTreeMap::new(),
        ),
        (
            "compare",
            "compare",
            vec![("f32", json!(-0.0_f32)), ("f32", json!(0.0_f32))],
            BTreeMap::from([("predicate".to_owned(), json!("eq"))]),
        ),
        (
            "cast",
            "cast",
            vec![("i32", json!(-17))],
            BTreeMap::from([("target_type".to_owned(), json!("f32"))]),
        ),
        (
            "select",
            "select",
            vec![("bool", json!(true)), ("i32", json!(-4)), ("i32", json!(9))],
            BTreeMap::new(),
        ),
    ];
    let mut hashes = Vec::new();
    for (label, opcode, constants, attributes) in cases {
        let mut workspace = constant_candidate(label, opcode, &constants, attributes);
        let rewritten =
            apply_first_match(&mut workspace.0, &workspace.1, FOLD_SCALAR_CONSTANTS_RULE);
        validate_current(&workspace.0, &rewritten, seed);
        hashes.push(rewritten.impl_hash.to_string());
        if label == "add-i32" {
            let pruned = apply_first_match(&mut workspace.0, &rewritten, PRUNE_UNREACHABLE_RULE);
            assert_eq!(pruned.impl_hash, rewritten.impl_hash);
            validate_current(&workspace.0, &pruned, seed);
            hashes.push(pruned.candidate_hash.to_string());
        }
    }
    hashes
}

#[test]
fn fixed_seed_oracle_confirms_every_production_known_rewrite() {
    assert_eq!(
        fixed_seed_soundness_sequence(0x5eed),
        fixed_seed_soundness_sequence(0x5eed)
    );

    let id = WorkspaceId::new("noop-cast-soundness");
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
                    attributes: BTreeMap::from([("target_type".to_owned(), json!("f32"))]),
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
    freeze(&mut workspace);
    let identity = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let eliminated = apply_first_match(&mut workspace, &identity, ELIMINATE_NOOP_CAST_RULE);
    validate_current(&workspace, &eliminated, 0x5eed);
}

#[test]
fn differential_tensor_limit_precedes_generated_tensor_materialization() {
    let id = WorkspaceId::new("differential-limit");
    let mut workspace = Workspace::new(id.clone()).unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "tensor<f32,[4]>".parse().unwrap(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$x".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    freeze(&mut workspace);
    let candidate = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    let source = &workspace.revision(&RevisionId::new("r2")).unwrap().program;
    let implementation = &workspace
        .candidate_revision(&candidate.candidate, &candidate.candidate_revision)
        .unwrap()
        .impl_program;
    let limits = ResourceLimits {
        differential_tensor_elements: 3,
        ..ResourceLimits::default()
    };
    let error = differential_validate(source, implementation, 1, 1, &limits)
        .expect_err("four elements exceed the limit before materialization");
    assert_eq!(error.code, agentir_core::ErrorCode::ResourceLimitExceeded);

    let exact_limits = ResourceLimits {
        differential_tensor_elements: 4,
        ..ResourceLimits::default()
    };
    assert!(
        differential_validate(source, implementation, 1, 1, &exact_limits)
            .unwrap()
            .passed
    );
}
