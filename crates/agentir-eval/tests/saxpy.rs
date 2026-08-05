use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionOpSpec, RegionSpec},
};
use agentir_eval::evaluate;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn saxpy_end_to_end() {
    let mut workspace = Workspace::new(WorkspaceId::new("w1")).expect("workspace");
    let build_transaction = Transaction {
        workspace: WorkspaceId::new("w1"),
        base_revision: RevisionId::new("r0"),
        actions: vec![
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
                        bind: "$out".to_owned(),
                        opcode: "fma".to_owned(),
                        operands: vec!["$a".to_owned(), "xi".to_owned(), "yi".to_owned()],
                        attributes: BTreeMap::new(),
                    }],
                    yield_value: "$out".to_owned(),
                }),
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$out".to_owned(),
            },
        ],
        client_transaction_id: Some("build-saxpy".to_owned()),
        allow_branch: false,
    };
    let build_commit = workspace.apply(&build_transaction).expect("build commits");
    let freeze = Transaction {
        workspace: WorkspaceId::new("w1"),
        base_revision: build_commit.revision,
        actions: vec![Action::FreezeSpec],
        client_transaction_id: None,
        allow_branch: false,
    };
    let frozen = workspace.apply(&freeze).expect("freeze commits");
    let inputs = BTreeMap::from([
        ("a".to_owned(), json!(2.0)),
        ("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0])),
        ("y".to_owned(), json!([10.0, 20.0, 30.0, 40.0])),
    ]);
    let result = evaluate(
        &workspace
            .revision(&frozen.revision)
            .expect("revision")
            .program,
        &inputs,
    )
    .expect("evaluates");
    assert_eq!(result.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
    assert_eq!(result.dimensions["N"], 4);
}
