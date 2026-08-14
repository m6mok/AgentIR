use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, GRAPH_SCHEMA, GraphOpcode, GraphOperand,
    GraphOperation, GraphProposal, TASK_SCHEMA, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn scalar(name: &str) -> GraphOperand {
    GraphOperand::Scalar {
        name: name.to_owned(),
    }
}

fn tensor(name: &str) -> GraphOperand {
    GraphOperand::Tensor {
        name: name.to_owned(),
    }
}

const fn local(operation: usize) -> GraphOperand {
    GraphOperand::Local { operation }
}

fn operation(op: GraphOpcode, operands: Vec<GraphOperand>) -> GraphOperation {
    GraphOperation { op, operands }
}

fn publish(
    task_id: &str,
    scalars: &[&str],
    tensors: &[&str],
    inputs: Value,
    intent: GraphProposal,
    proposal: &GraphProposal,
) {
    assert_eq!(proposal, &intent);
    let result = AuthoringGateway::new()
        .publish(
            &AuthoringTask {
                schema: TASK_SCHEMA.to_owned(),
                task_id: task_id.to_owned(),
                dimension: "N".to_owned(),
                scalars: scalars.iter().map(|name| (*name).to_owned()).collect(),
                tensors: tensors.iter().map(|name| (*name).to_owned()).collect(),
                inputs: serde_json::from_value::<BTreeMap<String, Value>>(inputs)
                    .expect("named inputs"),
                intent,
            },
            proposal,
            ExecutionMode::Portable,
        )
        .expect("exact weaker-agent graph publishes and executes");
    let outputs = result.outputs["out"].as_array().expect("tensor output");
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(Value::is_number));
}

#[test]
fn terra_low_preserves_exact_schema_fma_order_and_dag_reuse_on_small_graph() {
    let intent = GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations: vec![
            operation(
                GraphOpcode::Fma,
                vec![tensor("signal"), scalar("gain"), tensor("offset")],
            ),
            operation(GraphOpcode::Mul, vec![tensor("mask"), local(0)]),
            operation(GraphOpcode::Add, vec![local(1), tensor("residual")]),
            operation(GraphOpcode::Mul, vec![local(0), local(2)]),
            operation(GraphOpcode::Add, vec![tensor("baseline"), local(3)]),
        ],
        r#yield: 4,
    };
    let proposal = parse_proposal(include_str!("fixtures/terra_low_small.json"))
        .expect("small first attempt follows schema");
    publish(
        "terra-low-small",
        &["gain"],
        &["signal", "offset", "mask", "residual", "baseline"],
        json!({
            "gain":0.5,"signal":[0.1,0.2],"offset":[0.01,0.02],
            "mask":[0.3,0.4],"residual":[0.05,0.06],"baseline":[0.07,0.08]
        }),
        intent,
        &proposal,
    );
}

fn recurrence_32() -> GraphProposal {
    let mut operations = vec![
        operation(GraphOpcode::Mul, vec![scalar("a0"), tensor("x0")]),
        operation(
            GraphOpcode::Fma,
            vec![tensor("x1"), scalar("a1"), tensor("x2")],
        ),
        operation(GraphOpcode::Add, vec![local(0), local(1)]),
        operation(GraphOpcode::Mul, vec![tensor("x3"), local(2)]),
        operation(GraphOpcode::Fma, vec![scalar("a2"), tensor("x4"), local(3)]),
        operation(GraphOpcode::Add, vec![tensor("x5"), local(4)]),
        operation(GraphOpcode::Mul, vec![local(2), local(5)]),
        operation(GraphOpcode::Fma, vec![local(6), scalar("a3"), tensor("x6")]),
    ];
    for index in 8..32 {
        operations.push(match index % 4 {
            0 => operation(GraphOpcode::Add, vec![local(index - 1), local(index - 8)]),
            1 => operation(
                GraphOpcode::Mul,
                vec![tensor(&format!("x{}", index % 8)), local(index - 1)],
            ),
            2 => operation(
                GraphOpcode::Fma,
                vec![
                    scalar(&format!("a{}", (index / 4) % 4)),
                    local(index - 1),
                    local(index - 6),
                ],
            ),
            3 => operation(
                GraphOpcode::Add,
                vec![local(index - 1), tensor(&format!("x{}", (index + 2) % 8))],
            ),
            _ => unreachable!(),
        });
    }
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: 31,
    }
}

#[test]
fn terra_low_expands_and_executes_exact_32_operation_recurrence() {
    let intent = recurrence_32();
    let proposal = parse_proposal(include_str!("fixtures/terra_low_32.json"))
        .expect("32-operation first attempt follows schema");
    publish(
        "terra-low-32",
        &["a0", "a1", "a2", "a3"],
        &["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7"],
        json!({
            "a0":0.1,"a1":0.2,"a2":0.3,"a3":0.4,
            "x0":[0.01,0.02],"x1":[0.02,0.03],"x2":[0.03,0.04],
            "x3":[0.04,0.05],"x4":[0.05,0.06],"x5":[0.06,0.07],
            "x6":[0.07,0.08],"x7":[0.08,0.09]
        }),
        intent,
        &proposal,
    );
}
