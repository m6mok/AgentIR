use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionOpSpec, RegionSpec},
    semantic::{SPEC_CANONICAL_VERSION, canonicalize_spec},
    types::{Determinism, FmaPolicy},
};
use serde_json::json;
use std::collections::BTreeMap;

fn apply(workspace: &mut Workspace, actions: Vec<Action>) -> RevisionId {
    let base_revision = workspace.head().clone();
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision,
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("transaction commits")
        .revision
}

fn saxpy(
    workspace_name: &str,
    dimension: &str,
    parameter_order: &[&str],
    argument_names: [&str; 2],
    local_name: &str,
    unreachable_prefix: usize,
) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(workspace_name)).expect("workspace");
    let mut actions = vec![Action::DefineDimension {
        bind: Some("$dimension".to_owned()),
        name: dimension.to_owned(),
        constraints: vec![format!("{dimension} >= 0")],
    }];
    for index in 0..unreachable_prefix {
        actions.push(Action::CreateConstant {
            bind: format!("$unused{index}"),
            ty: "f32".parse().expect("type"),
            value: json!(index as f32 + 0.5),
        });
    }
    if unreachable_prefix > 0 {
        actions.push(Action::CreateOp {
            bind: "$unused_op".to_owned(),
            opcode: "add".to_owned(),
            operands: vec!["$unused0".to_owned(), "$unused0".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
    }
    for name in parameter_order {
        let ty = if *name == "a" {
            "f32".parse().expect("type")
        } else {
            format!("tensor<f32,[{dimension}]>").parse().expect("type")
        };
        actions.push(Action::CreateParameter {
            bind: format!("${name}"),
            name: (*name).to_owned(),
            ty,
        });
    }
    actions.extend([
        Action::CreateOp {
            bind: "$out".to_owned(),
            opcode: "zip_map".to_owned(),
            operands: vec!["$x".to_owned(), "$y".to_owned()],
            attributes: BTreeMap::new(),
            region: Some(RegionSpec {
                arguments: vec![
                    RegionArgumentSpec {
                        name: argument_names[0].to_owned(),
                        ty: "f32".parse().expect("type"),
                    },
                    RegionArgumentSpec {
                        name: argument_names[1].to_owned(),
                        ty: "f32".parse().expect("type"),
                    },
                ],
                captures: vec!["$a".to_owned(), "$x".to_owned()],
                operations: vec![RegionOpSpec {
                    bind: local_name.to_owned(),
                    opcode: "fma".to_owned(),
                    operands: vec![
                        "$a".to_owned(),
                        argument_names[0].to_owned(),
                        argument_names[1].to_owned(),
                    ],
                    attributes: BTreeMap::new(),
                }],
                yield_value: local_name.to_owned(),
            }),
        },
        Action::SetOutput {
            name: "out".to_owned(),
            value: "$out".to_owned(),
        },
    ]);
    apply(&mut workspace, actions);
    apply(&mut workspace, vec![Action::FreezeSpec]);
    workspace
}

fn binary_program(opcode: &str, operands: [&str; 2]) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new("binary")).expect("workspace");
    apply(
        &mut workspace,
        vec![
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "f32".parse().expect("type"),
            },
            Action::CreateParameter {
                bind: "$y".to_owned(),
                name: "y".to_owned(),
                ty: "f32".parse().expect("type"),
            },
            Action::CreateOp {
                bind: "$result".to_owned(),
                opcode: opcode.to_owned(),
                operands: operands.into_iter().map(str::to_owned).collect(),
                attributes: BTreeMap::new(),
                region: None,
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$result".to_owned(),
            },
        ],
    );
    apply(&mut workspace, vec![Action::FreezeSpec]);
    workspace
}

fn independent_branches(right_first: bool) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new("branches")).expect("workspace");
    let mut actions = vec![
        Action::CreateParameter {
            bind: "$x".to_owned(),
            name: "x".to_owned(),
            ty: "f32".parse().expect("type"),
        },
        Action::CreateParameter {
            bind: "$y".to_owned(),
            name: "y".to_owned(),
            ty: "f32".parse().expect("type"),
        },
    ];
    let left = Action::CreateOp {
        bind: "$left".to_owned(),
        opcode: "add".to_owned(),
        operands: vec!["$x".to_owned(), "$y".to_owned()],
        attributes: BTreeMap::new(),
        region: None,
    };
    let right = Action::CreateOp {
        bind: "$right".to_owned(),
        opcode: "mul".to_owned(),
        operands: vec!["$x".to_owned(), "$y".to_owned()],
        attributes: BTreeMap::new(),
        region: None,
    };
    if right_first {
        actions.extend([right, left]);
    } else {
        actions.extend([left, right]);
    }
    actions.extend([
        Action::CreateOp {
            bind: "$result".to_owned(),
            opcode: "sub".to_owned(),
            operands: vec!["$left".to_owned(), "$right".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        },
        Action::SetOutput {
            name: "out".to_owned(),
            value: "$result".to_owned(),
        },
    ]);
    apply(&mut workspace, actions);
    apply(&mut workspace, vec![Action::FreezeSpec]);
    workspace
}

fn semantic(workspace: &Workspace) -> agentir_core::semantic::SemanticCanonicalization {
    workspace
        .semantic_canonical(workspace.head())
        .expect("semantic canonical form")
}

#[test]
fn saxpy_id_histories_have_distinct_content_and_equal_semantic_hashes() {
    let plain = saxpy("plain", "N", &["a", "x", "y"], ["xi", "yi"], "$r", 0);
    let offset = saxpy("offset", "N", &["a", "x", "y"], ["xi", "yi"], "$r", 7);
    assert_ne!(
        plain.revision(plain.head()).expect("revision").content_hash,
        offset
            .revision(offset.head())
            .expect("revision")
            .content_hash
    );
    assert_eq!(semantic(&plain).spec_hash, semantic(&offset).spec_hash);
}

#[test]
fn construction_order_unreachable_graph_and_region_names_do_not_change_semantics() {
    let left = saxpy("left", "N", &["a", "x", "y"], ["xi", "yi"], "$sum", 0);
    let right = saxpy(
        "right",
        "N",
        &["y", "a", "x"],
        ["lhs", "rhs"],
        "$different",
        3,
    );
    assert_eq!(semantic(&left).canonical, semantic(&right).canonical);

    let left_first = independent_branches(false);
    let right_first = independent_branches(true);
    assert_eq!(
        semantic(&left_first).spec_hash,
        semantic(&right_first).spec_hash
    );
}

#[test]
fn symbolic_dimensions_alpha_normalize() {
    let n = saxpy("n", "N", &["a", "x", "y"], ["x", "y"], "$r", 0);
    let length = saxpy("length", "Length", &["a", "x", "y"], ["x", "y"], "$r", 0);
    assert_eq!(semantic(&n).spec_hash, semantic(&length).spec_hash);
}

#[test]
fn external_interface_names_and_unused_parameters_remain_semantic() {
    let base = saxpy("base", "N", &["a", "x", "y"], ["x", "y"], "$r", 0);
    let extra = saxpy(
        "extra",
        "N",
        &["a", "x", "y", "unused"],
        ["x", "y"],
        "$r",
        0,
    );
    assert_ne!(semantic(&base).spec_hash, semantic(&extra).spec_hash);

    let mut renamed_output = base
        .revision(base.head())
        .expect("revision")
        .program
        .clone();
    let value = renamed_output.outputs.remove("out").expect("output");
    renamed_output.outputs.insert("renamed".to_owned(), value);
    assert_ne!(
        semantic(&base).spec_hash,
        canonicalize_spec(&renamed_output)
            .expect("renamed output")
            .spec_hash
    );
}

#[test]
fn opcode_and_non_commutative_operand_order_change_semantics() {
    let add = binary_program("add", ["$x", "$y"]);
    let sub = binary_program("sub", ["$x", "$y"]);
    let reversed_sub = binary_program("sub", ["$y", "$x"]);
    assert_ne!(semantic(&add).spec_hash, semantic(&sub).spec_hash);
    assert_ne!(semantic(&sub).spec_hash, semantic(&reversed_sub).spec_hash);
}

fn constant_program(bits: &str) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new("constant")).expect("workspace");
    apply(
        &mut workspace,
        vec![
            Action::CreateConstant {
                bind: "$value".to_owned(),
                ty: "f32".parse().expect("type"),
                value: json!(bits),
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$value".to_owned(),
            },
        ],
    );
    apply(&mut workspace, vec![Action::FreezeSpec]);
    workspace
}

#[test]
fn constant_bits_and_numeric_contract_change_semantics() {
    let one = constant_program("0x3f800000");
    let next = constant_program("0x3f800001");
    assert_ne!(semantic(&one).spec_hash, semantic(&next).spec_hash);

    let mut changed = one.revision(one.head()).expect("revision").program.clone();
    changed.numeric_contract.fma = FmaPolicy::Required;
    changed.numeric_contract.reassociation = true;
    changed.numeric_contract.determinism = Determinism::NotRequired;
    assert_ne!(
        semantic(&one).spec_hash,
        canonicalize_spec(&changed).expect("canonical").spec_hash
    );
}

#[test]
fn repeated_canonicalization_is_byte_for_byte_stable() {
    let workspace = saxpy("stable", "N", &["a", "x", "y"], ["x", "y"], "$r", 2);
    let program = &workspace
        .revision(workspace.head())
        .expect("revision")
        .program;
    let first = canonicalize_spec(program).expect("first");
    let second = canonicalize_spec(program).expect("second");
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.spec_hash, second.spec_hash);
    assert_eq!(first.canonical.version, SPEC_CANONICAL_VERSION);
}

#[test]
fn unresolved_persistent_references_in_attributes_are_structured_failures() {
    let workspace = binary_program("add", ["$x", "$y"]);
    let mut program = workspace
        .revision(workspace.head())
        .expect("revision")
        .program
        .clone();
    program
        .operations
        .values_mut()
        .find(|operation| operation.opcode.to_string() == "add")
        .expect("add")
        .attributes
        .insert("unstable_reference".to_owned(), json!("v1"));
    let error = canonicalize_spec(&program).expect_err("persistent reference rejected");
    assert_eq!(error.code, ErrorCode::CanonicalizationFailed);
}

#[test]
fn drafts_have_no_semantic_hash_and_query_is_structured_error() {
    let mut workspace = Workspace::new(WorkspaceId::new("draft")).expect("workspace");
    apply(
        &mut workspace,
        vec![
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
    );
    let revision = workspace.revision(workspace.head()).expect("revision");
    assert!(revision.spec_hash.is_none());
    assert!(revision.semantic_canonical_version.is_none());
    let error = workspace
        .semantic_canonical(workspace.head())
        .expect_err("draft is not canonicalizable");
    assert_eq!(error.code, ErrorCode::SpecNotComplete);
}

#[test]
fn fixed_seed_history_permutations_converge() {
    let reference = saxpy(
        "seed-0",
        "D0",
        &["a", "x", "y"],
        ["arg0", "arg1"],
        "$local0",
        0,
    );
    let expected = semantic(&reference).spec_hash;
    let permutations = [
        ["a", "x", "y"],
        ["a", "y", "x"],
        ["x", "a", "y"],
        ["x", "y", "a"],
        ["y", "a", "x"],
        ["y", "x", "a"],
    ];
    let mut state = 0x5eed_u64;
    for case in 0..48 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let order = permutations[(state as usize) % permutations.len()];
        let dimension = format!("Dimension{case}");
        let first = format!("left{case}");
        let second = format!("right{case}");
        let local = format!("$result{case}");
        let candidate = saxpy(
            &format!("seed-{case}"),
            &dimension,
            &order,
            [&first, &second],
            &local,
            (state as usize >> 8) % 6,
        );
        assert_eq!(semantic(&candidate).spec_hash, expected, "case {case}");
    }
}
