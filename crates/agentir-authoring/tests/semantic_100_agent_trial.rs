use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, GRAPH_SCHEMA, GraphOpcode, GraphOperand,
    GraphOperation, GraphProposal, TASK_SCHEMA, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn scalar(index: usize) -> GraphOperand {
    GraphOperand::Scalar {
        name: format!("s{index}"),
    }
}

fn tensor(index: usize) -> GraphOperand {
    GraphOperand::Tensor {
        name: format!("x{index}"),
    }
}

const fn local(operation: usize) -> GraphOperand {
    GraphOperand::Local { operation }
}

fn operation(op: GraphOpcode, operands: Vec<GraphOperand>) -> GraphOperation {
    GraphOperation { op, operands }
}

fn expected_semantic_100_graph() -> GraphProposal {
    let mut operations = vec![
        operation(GraphOpcode::Mul, vec![scalar(0), tensor(0)]),
        operation(GraphOpcode::Fma, vec![tensor(1), scalar(1), tensor(2)]),
        operation(GraphOpcode::Add, vec![local(0), local(1)]),
        operation(GraphOpcode::Mul, vec![tensor(3), local(2)]),
        operation(GraphOpcode::Fma, vec![scalar(2), tensor(4), local(3)]),
        operation(GraphOpcode::Add, vec![tensor(5), local(4)]),
        operation(GraphOpcode::Mul, vec![local(2), local(5)]),
        operation(GraphOpcode::Fma, vec![local(6), scalar(3), tensor(6)]),
        operation(GraphOpcode::Add, vec![local(7), local(0)]),
        operation(GraphOpcode::Mul, vec![scalar(4), local(8)]),
    ];
    for index in 10..100 {
        operations.push(match index % 5 {
            0 => operation(GraphOpcode::Add, vec![local(index - 1), local(index - 10)]),
            1 => operation(GraphOpcode::Mul, vec![local(index - 1), tensor(index % 10)]),
            2 => operation(
                GraphOpcode::Fma,
                vec![local(index - 1), scalar((index / 5) % 5), local(index - 7)],
            ),
            3 => operation(
                GraphOpcode::Add,
                vec![tensor((index + 3) % 10), local(index - 1)],
            ),
            4 => operation(
                GraphOpcode::Mul,
                vec![scalar(((index / 5) + 1) % 5), local(index - 1)],
            ),
            _ => unreachable!(),
        });
    }
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: 99,
    }
}

#[test]
fn high_capability_agent_expands_and_executes_exact_100_operation_recurrence() {
    let agent_output = parse_proposal(include_str!("fixtures/semantic_100_good_agent_output.json"))
        .expect("agent output follows strict schema");
    let expected = expected_semantic_100_graph();
    assert_eq!(agent_output, expected);
    assert_eq!(agent_output.operations.len(), 100);
    assert_eq!(
        agent_output
            .operations
            .iter()
            .filter(|operation| operation.op == GraphOpcode::Fma)
            .count(),
        21
    );

    let mut inputs = BTreeMap::new();
    for index in 0..5 {
        inputs.insert(
            format!("s{index}"),
            json!(0.01 + (f64::from(index) * 0.005)),
        );
    }
    for index in 0..10 {
        let first = 0.01 + (f64::from(index) * 0.002);
        inputs.insert(format!("x{index}"), json!([first, first + 0.001]));
    }
    let task = AuthoringTask {
        schema: TASK_SCHEMA.to_owned(),
        task_id: "semantic-100-good-agent".to_owned(),
        dimension: "N".to_owned(),
        scalars: (0..5).map(|index| format!("s{index}")).collect(),
        tensors: (0..10).map(|index| format!("x{index}")).collect(),
        inputs,
        intent: expected,
    };
    let result = AuthoringGateway::new()
        .publish(&task, &agent_output, ExecutionMode::Portable)
        .expect("100-operation graph publishes and executes");
    let outputs = result.outputs["out"].as_array().expect("tensor output");
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(Value::is_number));
    assert_eq!(result.model_visible_calls, 1);
    assert_eq!(result.internal_agentir_requests, 10);
}
