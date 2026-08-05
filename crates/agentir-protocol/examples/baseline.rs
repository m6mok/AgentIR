//! Small dependency-free local performance baseline.

use agentir_core::{
    Action, HoleId, RevisionId, Transaction, Workspace, WorkspaceId, canonical::canonical_bytes,
    continuation::InteractionMode, shapes::same_shape, types::Shape,
};
use agentir_protocol::Engine;
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
            "continuation_generation_10k": continuation,
            "saxpy_evaluation_by_elements": evaluation,
        })
    );
}
