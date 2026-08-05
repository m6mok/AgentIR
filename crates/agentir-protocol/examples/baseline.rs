//! Small dependency-free local performance baseline.

use agentir_core::{
    Action, HoleId, RevisionId, Transaction, Workspace, WorkspaceId, canonical::canonical_bytes,
    continuation::InteractionMode, semantic::canonicalize_spec, shapes::same_shape, types::Shape,
};
use agentir_protocol::Engine;
use agentir_store::{WorkspaceArchiveV1, load_workspace_bytes, migrate_archive_v1_to_v2};
use serde_json::json;
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn elapsed_ns(mut operation: impl FnMut()) -> u128 {
    let started = Instant::now();
    operation();
    started.elapsed().as_nanos()
}

fn transaction_apply(operation_count: usize) -> u128 {
    let mut workspace = Workspace::new(WorkspaceId::new("bench")).expect("workspace");
    let mut actions = vec![
        Action::CreateConstant {
            bind: "$acc0".to_owned(),
            ty: "f32".parse().expect("type"),
            value: json!(1.0),
        },
        Action::CreateConstant {
            bind: "$one".to_owned(),
            ty: "f32".parse().expect("type"),
            value: json!(1.0),
        },
    ];
    for index in 0..operation_count {
        actions.push(Action::CreateOp {
            bind: format!("$acc{}", index + 1),
            opcode: "add".to_owned(),
            operands: vec![format!("$acc{index}"), "$one".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$acc{operation_count}"),
    });
    let transaction = Transaction {
        workspace: WorkspaceId::new("bench"),
        base_revision: RevisionId::new("r0"),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    };
    elapsed_ns(|| {
        black_box(workspace.apply(&transaction).expect("transaction"));
    })
}

fn frozen_chain(operation_count: usize) -> Workspace {
    let mut workspace =
        Workspace::new(WorkspaceId::new(format!("chain-{operation_count}"))).expect("workspace");
    let mut actions = vec![
        Action::CreateParameter {
            bind: "$acc0".to_owned(),
            name: "input".to_owned(),
            ty: "f32".parse().expect("type"),
        },
        Action::CreateConstant {
            bind: "$one".to_owned(),
            ty: "f32".parse().expect("type"),
            value: json!(1.0),
        },
    ];
    for index in 0..operation_count {
        actions.push(Action::CreateOp {
            bind: format!("$acc{}", index + 1),
            opcode: "add".to_owned(),
            operands: vec![format!("$acc{index}"), "$one".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$acc{operation_count}"),
    });
    let built = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("build");
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .expect("freeze");
    workspace
}

fn main() {
    let apply = [1_usize, 10, 100]
        .into_iter()
        .map(|count| (count.to_string(), transaction_apply(count)))
        .collect::<BTreeMap<_, _>>();

    let left: Shape = "[M,2*N+1]".parse().expect("shape");
    let right = left.clone();
    let shape_query = elapsed_ns(|| {
        for _ in 0..100_000 {
            black_box(same_shape(&left, &right));
        }
    });

    let mut workspace = Workspace::new(WorkspaceId::new("canonical")).expect("workspace");
    let partial = Transaction {
        workspace: WorkspaceId::new("canonical"),
        base_revision: RevisionId::new("r0"),
        actions: vec![
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
        client_transaction_id: None,
        allow_branch: false,
    };
    let revision = workspace.apply(&partial).expect("partial").revision;
    let program = workspace
        .revision(&revision)
        .expect("revision")
        .program
        .clone();
    let canonical = elapsed_ns(|| {
        for _ in 0..10_000 {
            black_box(canonical_bytes(&program).expect("canonical"));
        }
    });

    let semantic_workspaces = [10_usize, 100, 1_000]
        .into_iter()
        .map(|count| (count, frozen_chain(count)))
        .collect::<BTreeMap<_, _>>();
    let semantic_canonicalization = semantic_workspaces
        .iter()
        .map(|(count, workspace)| {
            let program = &workspace
                .revision(workspace.head())
                .expect("revision")
                .program;
            (
                count.to_string(),
                elapsed_ns(|| {
                    black_box(canonicalize_spec(program).expect("semantic canonicalization"));
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let semantic_canonical_bytes = semantic_workspaces
        .iter()
        .map(|(count, workspace)| {
            let program = &workspace
                .revision(workspace.head())
                .expect("revision")
                .program;
            (
                count.to_string(),
                canonicalize_spec(program)
                    .expect("semantic canonicalization")
                    .bytes
                    .len(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let repeated_spec_hash_query = {
        let workspace = semantic_workspaces.get(&100).expect("100-op chain");
        elapsed_ns(|| {
            for _ in 0..1_000 {
                black_box(
                    workspace
                        .semantic_canonical(workspace.head())
                        .expect("semantic query")
                        .spec_hash,
                );
            }
        })
    };

    let legacy: WorkspaceArchiveV1 = serde_json::from_slice(include_bytes!(
        "../../agentir-store/tests/fixtures/minimal-v1.json"
    ))
    .expect("v1 fixture");
    let v1_to_v2_migration = elapsed_ns(|| {
        for _ in 0..100 {
            black_box(migrate_archive_v1_to_v2(legacy.clone()).expect("migration"));
        }
    });
    let v2_archive = include_bytes!("../../agentir-store/tests/fixtures/minimal-v2.json");
    let v2_archive_load_replay = elapsed_ns(|| {
        for _ in 0..100 {
            black_box(load_workspace_bytes(v2_archive).expect("v2 load"));
        }
    });
    let continuation = elapsed_ns(|| {
        for _ in 0..10_000 {
            black_box(
                workspace
                    .continuation(&revision, &HoleId::new("h1"), InteractionMode::Hybrid)
                    .expect("continuation"),
            );
        }
    });

    let mut engine = Engine::new();
    for line in include_str!("../../../examples/saxpy.jsonl")
        .lines()
        .take(4)
    {
        let response = engine.process_line(line);
        assert!(response.contains("\"ok\":true"), "{response}");
    }
    let evaluation = [4_usize, 1_024, 65_536]
        .into_iter()
        .map(|size| {
            let x = (0..size).map(|value| value as f32).collect::<Vec<_>>();
            let y = vec![1.0_f32; size];
            let request = json!({
                "command": "program.evaluate",
                "request_id": format!("eval-{size}"),
                "workspace": "w1",
                "revision": "r2",
                "inputs": {"a": 2.0, "x": x, "y": y},
            })
            .to_string();
            let elapsed = elapsed_ns(|| {
                let response = engine.process_line(&request);
                assert!(response.contains("\"ok\":true"), "{response}");
                black_box(response);
            });
            (size.to_string(), elapsed)
        })
        .collect::<BTreeMap<_, _>>();

    println!(
        "{}",
        json!({
            "unit": "nanoseconds",
            "transaction_apply_by_operation_count": apply,
            "shape_query_100k": shape_query,
            "canonical_serialization_10k": canonical,
            "semantic_canonicalization_by_reachable_operations": semantic_canonicalization,
            "semantic_canonical_bytes_by_reachable_operations": semantic_canonical_bytes,
            "repeated_spec_hash_query_1k": repeated_spec_hash_query,
            "v1_to_v2_migration_100": v1_to_v2_migration,
            "v2_archive_load_replay_100": v2_archive_load_replay,
            "continuation_generation_10k": continuation,
            "saxpy_evaluation_by_elements": evaluation,
        })
    );
}
