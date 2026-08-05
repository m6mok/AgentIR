use agentir_core::{
    Action, ErrorCode, HoleId, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{ActionClassification, RegionArgumentSpec, RegionOpSpec, RegionSpec},
    continuation::InteractionMode,
    types::Type,
};
use serde_json::json;
use std::collections::BTreeMap;

fn transaction(base: &str, actions: Vec<Action>) -> Transaction {
    Transaction {
        workspace: WorkspaceId::new("w1"),
        base_revision: RevisionId::new(base),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    }
}

#[test]
fn rejected_transaction_is_atomic() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let invalid = transaction(
        "r0",
        vec![
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "f32".parse().expect("type"),
            },
            Action::CreateParameter {
                bind: "$flag".to_owned(),
                name: "flag".to_owned(),
                ty: "bool".parse().expect("type"),
            },
            Action::CreateOp {
                bind: "$bad".to_owned(),
                opcode: "add".to_owned(),
                operands: vec!["$x".to_owned(), "$flag".to_owned()],
                attributes: BTreeMap::new(),
                region: None,
            },
        ],
    );
    let error = workspace
        .apply(&invalid)
        .expect_err("invalid add is rejected");
    assert_eq!(error.code, ErrorCode::TypeMismatch);
    assert_eq!(workspace.head(), &RevisionId::new("r0"));
    assert!(workspace.revision(&RevisionId::new("r1")).is_err());
    assert_eq!(
        workspace
            .revision(&RevisionId::new("r0"))
            .expect("root")
            .program
            .values
            .len(),
        0
    );
}

#[test]
fn temporary_bindings_resolve_and_regions_are_checked() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let build = transaction(
        "r0",
        vec![
            Action::DefineDimension {
                bind: Some("$N".to_owned()),
                name: "N".to_owned(),
                constraints: vec!["N >= 0".to_owned()],
            },
            Action::CreateParameter {
                bind: "$a".to_owned(),
                name: "a".to_owned(),
                ty: "f32".parse().expect("type"),
            },
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "tensor<f32,[N]>".parse().expect("type"),
            },
            Action::CreateParameter {
                bind: "$y".to_owned(),
                name: "y".to_owned(),
                ty: "tensor<f32,[N]>".parse().expect("type"),
            },
            Action::CreateOp {
                bind: "$out".to_owned(),
                opcode: "zip_map".to_owned(),
                operands: vec!["$x".to_owned(), "$y".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(RegionSpec {
                    arguments: vec![
                        RegionArgumentSpec {
                            name: "xi".to_owned(),
                            ty: "f32".parse().expect("type"),
                        },
                        RegionArgumentSpec {
                            name: "yi".to_owned(),
                            ty: "f32".parse().expect("type"),
                        },
                    ],
                    captures: vec!["$a".to_owned()],
                    operations: vec![RegionOpSpec {
                        bind: "$sum".to_owned(),
                        opcode: "fma".to_owned(),
                        operands: vec!["$a".to_owned(), "xi".to_owned(), "yi".to_owned()],
                        attributes: BTreeMap::new(),
                    }],
                    yield_value: "$sum".to_owned(),
                }),
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$out".to_owned(),
            },
        ],
    );
    let result = workspace.apply(&build).expect("SAXPY graph commits");
    assert_eq!(result.bindings["$out"], "v4");
    assert_eq!(
        result.inferred["$out"],
        "tensor<f32,[N]>".parse::<Type>().expect("type")
    );
    assert!(
        result
            .classifications
            .iter()
            .all(|class| *class == ActionClassification::Legal)
    );
}

#[test]
fn holes_block_freeze_and_frame_filters_values() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let result = workspace
        .apply(&transaction(
            "r0",
            vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().expect("type"),
                },
                Action::CreateParameter {
                    bind: "$flag".to_owned(),
                    name: "flag".to_owned(),
                    ty: "bool".parse().expect("type"),
                },
                Action::CreateHole {
                    bind: "$hole".to_owned(),
                    expected_type: "f32".parse().expect("type"),
                    shape_constraints: Vec::new(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$hole".to_owned(),
                },
            ],
        ))
        .expect("partial graph commits");
    let frame = workspace
        .continuation(
            &result.revision,
            &HoleId::new("h1"),
            InteractionMode::Hybrid,
        )
        .expect("frame exists");
    let values = frame.slots[1].domain["values"]
        .as_array()
        .expect("value domain");
    assert_eq!(values, &[json!("v1")]);

    let freeze = transaction("r1", vec![Action::FreezeSpec]);
    let error = workspace
        .apply(&freeze)
        .expect_err("open hole blocks freeze");
    assert_eq!(error.code, ErrorCode::OpenHole);
    assert_eq!(workspace.head(), &RevisionId::new("r1"));
}

#[test]
fn explicit_forks_are_independent() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let base = workspace
        .apply(&transaction(
            "r0",
            vec![Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "f32".parse().expect("type"),
            }],
        ))
        .expect("base commits")
        .revision;
    let left = workspace.fork(&base).expect("left fork");
    let right = workspace.fork(&base).expect("right fork");
    assert_ne!(left, right);
    assert_eq!(
        workspace.revision(&left).expect("left").parents,
        vec![base.clone()]
    );
    assert_eq!(
        workspace.revision(&right).expect("right").parents,
        vec![base.clone()]
    );
    assert_eq!(
        workspace.revision(&left).expect("left").content_hash,
        workspace.revision(&right).expect("right").content_hash
    );
}

#[test]
fn frozen_spec_cannot_be_modified() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let built = workspace
        .apply(&transaction(
            "r0",
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
        ))
        .expect("build commits");
    let frozen = workspace
        .apply(&transaction(
            &built.revision.to_string(),
            vec![Action::FreezeSpec],
        ))
        .expect("freeze commits");
    let error = workspace
        .apply(&transaction(
            &frozen.revision.to_string(),
            vec![Action::CreateConstant {
                bind: "$one".to_owned(),
                ty: "f32".parse().expect("type"),
                value: json!(1.0),
            }],
        ))
        .expect_err("frozen graph rejects edits");
    assert_eq!(error.code, ErrorCode::SpecFrozen);
}

#[test]
fn compatible_value_fills_hole_and_allows_freeze() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let committed = workspace
        .apply(&transaction(
            "r0",
            vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().expect("type"),
                },
                Action::CreateHole {
                    bind: "$hole".to_owned(),
                    expected_type: "f32".parse().expect("type"),
                    shape_constraints: Vec::new(),
                },
                Action::FillHole {
                    hole: "$hole".to_owned(),
                    value: "$x".to_owned(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$hole".to_owned(),
                },
                Action::FreezeSpec,
            ],
        ))
        .expect("filled program freezes");
    let report = workspace.check(&committed.revision).expect("checks");
    assert!(report.deployable);
    assert!(report.open_holes.is_empty());
    assert!(report.open_obligations.is_empty());
}

#[test]
fn region_argument_mismatch_is_rejected_atomically() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let invalid = transaction(
        "r0",
        vec![
            Action::DefineDimension {
                bind: Some("$N".to_owned()),
                name: "N".to_owned(),
                constraints: vec!["N >= 0".to_owned()],
            },
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "tensor<f32,[N]>".parse().expect("type"),
            },
            Action::CreateOp {
                bind: "$bad".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["$x".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(RegionSpec {
                    arguments: vec![RegionArgumentSpec {
                        name: "xi".to_owned(),
                        ty: "bool".parse().expect("type"),
                    }],
                    captures: Vec::new(),
                    operations: Vec::new(),
                    yield_value: "xi".to_owned(),
                }),
            },
        ],
    );
    let error = workspace
        .apply(&invalid)
        .expect_err("invalid region is rejected");
    assert_eq!(error.code, ErrorCode::InvalidRegion);
    assert_eq!(workspace.head(), &RevisionId::new("r0"));
}

#[test]
fn snapshot_replay_restores_dag_hashes_and_allocator_state() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    let partial = workspace
        .apply(&transaction(
            "r0",
            vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().expect("type"),
                },
                Action::CreateHole {
                    bind: "$hole".to_owned(),
                    expected_type: "f32".parse().expect("type"),
                    shape_constraints: Vec::new(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$hole".to_owned(),
                },
            ],
        ))
        .expect("partial graph commits");
    let first_frame = workspace
        .continuation(
            &partial.revision,
            &HoleId::new("h1"),
            InteractionMode::Hybrid,
        )
        .expect("continuation");
    assert_eq!(first_frame.frame.to_string(), "cf1");
    let fork = workspace.fork(&partial.revision).expect("fork");
    assert_eq!(fork, RevisionId::new("r2"));

    let snapshot = workspace.snapshot();
    let (mut restored, report) = Workspace::from_snapshot(snapshot.clone()).expect("replays");
    assert_eq!(report.revisions_verified, 3);
    assert_eq!(report.events_replayed, 2);
    assert_eq!(report.content_hashes_verified, 3);
    assert_eq!(restored.snapshot(), snapshot);

    let next_frame = restored
        .continuation(&partial.revision, &HoleId::new("h1"), InteractionMode::Menu)
        .expect("restored allocator resumes");
    assert_eq!(next_frame.frame.to_string(), "cf2");
}

#[test]
fn snapshot_with_tampered_revision_hash_is_rejected() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace opens");
    workspace
        .apply(&transaction(
            "r0",
            vec![Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "f32".parse().expect("type"),
            }],
        ))
        .expect("transaction commits");
    let mut snapshot = workspace.snapshot();
    snapshot
        .revisions
        .get_mut(&RevisionId::new("r1"))
        .expect("revision")
        .content_hash = "tampered".to_owned();
    let error = Workspace::from_snapshot(snapshot).expect_err("tampering is rejected");
    assert_eq!(error.code, ErrorCode::PersistenceIntegrity);
}
