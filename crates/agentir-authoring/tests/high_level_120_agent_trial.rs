use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, GRAPH_SCHEMA, GraphOpcode, GraphOperand,
    GraphOperation, GraphProposal, TASK_SCHEMA, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn scalar(index: usize) -> GraphOperand {
    GraphOperand::Scalar {
        name: format!("g{index}"),
    }
}

fn tensor(prefix: &str, index: usize) -> GraphOperand {
    GraphOperand::Tensor {
        name: format!("{prefix}{index}"),
    }
}

const fn local(operation: usize) -> GraphOperand {
    GraphOperand::Local { operation }
}

fn operation(op: GraphOpcode, operands: Vec<GraphOperand>) -> GraphOperation {
    GraphOperation { op, operands }
}

fn expected_high_level_120_graph() -> GraphProposal {
    let mut operations = Vec::with_capacity(120);
    for stage in 0..12 {
        let base = stage * 10;
        let state = if stage == 0 {
            GraphOperand::Tensor {
                name: "seed".to_owned(),
            }
        } else {
            local(base - 1)
        };
        operations.extend([
            operation(
                GraphOpcode::Fma,
                vec![scalar(stage % 6), state.clone(), tensor("bias", stage % 4)],
            ),
            operation(
                GraphOpcode::Mul,
                vec![tensor("mask", ((3 * stage) + 1) % 4), local(base)],
            ),
            operation(
                GraphOpcode::Add,
                vec![local(base + 1), tensor("source", ((5 * stage) + 2) % 8)],
            ),
            operation(
                GraphOpcode::Fma,
                vec![local(base + 2), scalar((stage + 1) % 6), state.clone()],
            ),
            operation(
                GraphOpcode::Mul,
                vec![local(base + 3), tensor("gate", (stage + 2) % 4)],
            ),
            operation(GraphOpcode::Add, vec![state, local(base + 4)]),
            operation(
                GraphOpcode::Fma,
                vec![
                    scalar((stage + 2) % 6),
                    local(base + 5),
                    tensor("source", (stage + 3) % 8),
                ],
            ),
            operation(GraphOpcode::Mul, vec![local(base + 6), local(base)]),
            operation(GraphOpcode::Add, vec![local(base + 3), local(base + 7)]),
            operation(
                GraphOpcode::Add,
                vec![
                    local(base + 8),
                    if stage < 3 {
                        tensor("anchor", stage)
                    } else {
                        local(((stage - 3) * 10) + 9)
                    },
                ],
            ),
        ]);
    }
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: 119,
    }
}

#[test]
fn high_capability_agent_lowers_and_executes_exact_120_operation_mixer() {
    let agent_output = parse_proposal(include_str!(
        "fixtures/high_level_120_good_agent_output.json"
    ))
    .expect("agent output follows strict schema");
    let expected = expected_high_level_120_graph();
    assert_eq!(agent_output, expected);
    assert_eq!(agent_output.operations.len(), 120);
    assert_eq!(
        agent_output
            .operations
            .iter()
            .filter(|operation| operation.op == GraphOpcode::Fma)
            .count(),
        36
    );

    let mut inputs = BTreeMap::new();
    for index in 0..6 {
        inputs.insert(
            format!("g{index}"),
            json!(0.02 + (f64::from(index) * 0.003)),
        );
    }
    inputs.insert("seed".to_owned(), json!([0.01, 0.011]));
    for (prefix, count) in [
        ("bias", 4),
        ("mask", 4),
        ("source", 8),
        ("gate", 4),
        ("anchor", 3),
    ] {
        for index in 0..count {
            let first = 0.01 + (f64::from(index) * 0.001);
            inputs.insert(format!("{prefix}{index}"), json!([first, first + 0.0005]));
        }
    }
    let task = AuthoringTask {
        schema: TASK_SCHEMA.to_owned(),
        task_id: "high-level-120-good-agent".to_owned(),
        dimension: "N".to_owned(),
        scalars: (0..6).map(|index| format!("g{index}")).collect(),
        tensors: std::iter::once("seed".to_owned())
            .chain(
                [
                    ("bias", 4),
                    ("mask", 4),
                    ("source", 8),
                    ("gate", 4),
                    ("anchor", 3),
                ]
                .into_iter()
                .flat_map(|(prefix, count)| {
                    (0..count).map(move |index| format!("{prefix}{index}"))
                }),
            )
            .collect(),
        inputs,
        intent: expected,
    };
    let result = AuthoringGateway::new()
        .publish(&task, &agent_output, ExecutionMode::Portable)
        .expect("120-operation mixer publishes and executes");
    let outputs = result.outputs["out"].as_array().expect("tensor output");
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(Value::is_number));
    assert_eq!(result.model_visible_calls, 1);
    assert_eq!(result.internal_agentir_requests, 10);
}

#[test]
fn low_and_medium_trials_match_the_same_120_operation_oracle() {
    let expected = expected_high_level_120_graph();
    let trials = [
        ("low-1", include_str!("fixtures/mixer120_low_1.json")),
        (
            "low-audit-2",
            include_str!("fixtures/mixer120_low_audit_2.json"),
        ),
        (
            "low-audit-3",
            include_str!("fixtures/mixer120_low_audit_3.json"),
        ),
        ("medium-1", include_str!("fixtures/mixer120_medium_1.json")),
        ("medium-2", include_str!("fixtures/mixer120_medium_2.json")),
        ("medium-3", include_str!("fixtures/mixer120_medium_3.json")),
    ];
    for (name, payload) in trials {
        let proposal = parse_proposal(payload)
            .unwrap_or_else(|error| panic!("{name} must satisfy strict schema: {error}"));
        assert_eq!(proposal, expected, "{name} differs from canonical intent");
    }
}
