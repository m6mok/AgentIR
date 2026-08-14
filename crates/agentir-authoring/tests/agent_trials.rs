use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, TASK_SCHEMA, parse_proposal, parse_task,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn publish_agent_trial(task: &str, proposal: &str, expected: &Value) {
    let task = parse_task(task).expect("valid hidden server task");
    let proposal = parse_proposal(proposal).expect("first-attempt agent proposal follows schema");
    let result = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect("agent proposal matches intent and publishes");
    assert_eq!(&result.outputs["out"], expected);
    assert_eq!(result.model_visible_calls, 1);
    assert!(
        result
            .transcript
            .iter()
            .all(|entry| entry.response["ok"] == true)
    );
}

fn publish_recovered_agent_trial(
    task_id: &str,
    scalars: &[&str],
    tensors: &[&str],
    inputs: Value,
    proposal: &str,
    expected: &Value,
) {
    let proposal = parse_proposal(proposal).expect("one-shot repaired proposal follows schema");
    let task = AuthoringTask {
        schema: TASK_SCHEMA.to_owned(),
        task_id: task_id.to_owned(),
        dimension: "N".to_owned(),
        scalars: scalars.iter().map(|name| (*name).to_owned()).collect(),
        tensors: tensors.iter().map(|name| (*name).to_owned()).collect(),
        inputs: serde_json::from_value::<BTreeMap<String, Value>>(inputs).expect("named inputs"),
        intent: proposal.clone(),
    };
    let result = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect("recovered graph publishes");
    assert_eq!(&result.outputs["out"], expected);
}

#[test]
fn one_shot_repair_hint_recovers_three_observed_agent_failures() {
    publish_recovered_agent_trial(
        "recovery-a",
        &["a", "b"],
        &["x", "y"],
        json!({"a":2.0,"b":3.0,"x":[1.0,2.0],"y":[10.0,20.0]}),
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"a"},{"kind":"tensor","name":"x"}]},{"op":"mul","operands":[{"kind":"scalar","name":"b"},{"kind":"tensor","name":"y"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]}],"yield":2}"#,
        &json!([32.0, 64.0]),
    );
    publish_recovered_agent_trial(
        "recovery-b",
        &["gain"],
        &["signal", "offset", "mask"],
        json!({"gain":2.0,"signal":[1.0,2.0],"offset":[10.0,20.0],"mask":[3.0,4.0]}),
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"fma","operands":[{"kind":"scalar","name":"gain"},{"kind":"tensor","name":"signal"},{"kind":"tensor","name":"offset"}]},{"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"mask"}]}],"yield":1}"#,
        &json!([36.0, 96.0]),
    );
    publish_recovered_agent_trial(
        "recovery-c",
        &["p"],
        &["u", "v"],
        json!({"p":2.0,"u":[1.0,2.0],"v":[3.0,4.0]}),
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"p"},{"kind":"tensor","name":"u"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"v"}]},{"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]}],"yield":2}"#,
        &json!([10.0, 32.0]),
    );
}

#[test]
fn renamed_symbols_do_not_leak_names_from_the_wire_example() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch2-renamed",
            "dimension":"N",
            "scalars":["left_scale","right_scale"],
            "tensors":["primary","secondary"],
            "inputs":{
                "left_scale":2.0,"right_scale":3.0,
                "primary":[1.0,2.0],"secondary":[10.0,20.0]
            },
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"mul","operands":[{"kind":"scalar","name":"left_scale"},{"kind":"tensor","name":"primary"}]},
                    {"op":"mul","operands":[{"kind":"scalar","name":"right_scale"},{"kind":"tensor","name":"secondary"}]},
                    {"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]}
                ],
                "yield":2
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"left_scale"},{"kind":"tensor","name":"primary"}]},{"op":"mul","operands":[{"kind":"scalar","name":"right_scale"},{"kind":"tensor","name":"secondary"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]}],"yield":2}"#,
        &json!([32.0, 64.0]),
    );
}

#[test]
fn nested_fma_remains_fused_through_local_dependencies() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch2-nested-fma",
            "dimension":"N",
            "scalars":["gain"],
            "tensors":["signal","bias","mask","residual"],
            "inputs":{
                "gain":2.0,"signal":[1.0,2.0],"bias":[10.0,20.0],
                "mask":[3.0,4.0],"residual":[1.0,2.0]
            },
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"fma","operands":[{"kind":"scalar","name":"gain"},{"kind":"tensor","name":"signal"},{"kind":"tensor","name":"bias"}]},
                    {"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"mask"}]},
                    {"op":"add","operands":[{"kind":"local","operation":1},{"kind":"tensor","name":"residual"}]}
                ],
                "yield":2
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"fma","operands":[{"kind":"scalar","name":"gain"},{"kind":"tensor","name":"signal"},{"kind":"tensor","name":"bias"}]},{"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"mask"}]},{"op":"add","operands":[{"kind":"local","operation":1},{"kind":"tensor","name":"residual"}]}],"yield":2}"#,
        &json!([37.0, 98.0]),
    );
}

#[test]
fn six_operation_graph_keeps_every_dependency_and_operand_order() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch2-long-graph",
            "dimension":"N",
            "scalars":["alpha","beta","gamma"],
            "tensors":["x0","x1","x2","x3","x4"],
            "inputs":{
                "alpha":2.0,"beta":3.0,"gamma":0.5,
                "x0":[1.0,2.0],"x1":[10.0,20.0],"x2":[1.0,2.0],
                "x3":[2.0,3.0],"x4":[4.0,5.0]
            },
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"mul","operands":[{"kind":"scalar","name":"alpha"},{"kind":"tensor","name":"x0"}]},
                    {"op":"fma","operands":[{"kind":"scalar","name":"beta"},{"kind":"tensor","name":"x1"},{"kind":"tensor","name":"x2"}]},
                    {"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},
                    {"op":"mul","operands":[{"kind":"local","operation":2},{"kind":"tensor","name":"x3"}]},
                    {"op":"add","operands":[{"kind":"local","operation":3},{"kind":"tensor","name":"x4"}]},
                    {"op":"mul","operands":[{"kind":"local","operation":4},{"kind":"scalar","name":"gamma"}]}
                ],
                "yield":5
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"alpha"},{"kind":"tensor","name":"x0"}]},{"op":"fma","operands":[{"kind":"scalar","name":"beta"},{"kind":"tensor","name":"x1"},{"kind":"tensor","name":"x2"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},{"op":"mul","operands":[{"kind":"local","operation":2},{"kind":"tensor","name":"x3"}]},{"op":"add","operands":[{"kind":"local","operation":3},{"kind":"tensor","name":"x4"}]},{"op":"mul","operands":[{"kind":"local","operation":4},{"kind":"scalar","name":"gamma"}]}],"yield":5}"#,
        &json!([35.0, 101.5]),
    );
}

#[test]
fn explicit_nonconventional_operand_order_is_preserved() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch3-operand-order",
            "dimension":"N",
            "scalars":["factor"],
            "tensors":["samples","baseline"],
            "inputs":{"factor":2.0,"samples":[1.0,2.0],"baseline":[10.0,20.0]},
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"mul","operands":[{"kind":"tensor","name":"samples"},{"kind":"scalar","name":"factor"}]},
                    {"op":"add","operands":[{"kind":"tensor","name":"baseline"},{"kind":"local","operation":0}]}
                ],
                "yield":1
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"tensor","name":"samples"},{"kind":"scalar","name":"factor"}]},{"op":"add","operands":[{"kind":"tensor","name":"baseline"},{"kind":"local","operation":0}]}],"yield":1}"#,
        &json!([12.0, 24.0]),
    );
}

#[test]
fn reused_local_forms_a_dag_without_copy_or_renumbering() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch3-dag-reuse",
            "dimension":"N",
            "scalars":["scale"],
            "tensors":["x","y"],
            "inputs":{"scale":2.0,"x":[1.0,2.0],"y":[3.0,4.0]},
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"mul","operands":[{"kind":"scalar","name":"scale"},{"kind":"tensor","name":"x"}]},
                    {"op":"add","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"y"}]},
                    {"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},
                    {"op":"add","operands":[{"kind":"local","operation":2},{"kind":"local","operation":0}]}
                ],
                "yield":3
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"scale"},{"kind":"tensor","name":"x"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"y"}]},{"op":"mul","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},{"op":"add","operands":[{"kind":"local","operation":2},{"kind":"local","operation":0}]}],"yield":3}"#,
        &json!([12.0, 36.0]),
    );
}

#[test]
fn fma_multiplication_operands_are_not_reordered() {
    publish_agent_trial(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"batch3-fma-order",
            "dimension":"N",
            "scalars":["gain"],
            "tensors":["signal","offset"],
            "inputs":{"gain":2.0,"signal":[1.0,2.0],"offset":[10.0,20.0]},
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"fma","operands":[{"kind":"tensor","name":"signal"},{"kind":"scalar","name":"gain"},{"kind":"tensor","name":"offset"}]}
                ],
                "yield":0
            }
        }"#,
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"fma","operands":[{"kind":"tensor","name":"signal"},{"kind":"scalar","name":"gain"},{"kind":"tensor","name":"offset"}]}],"yield":0}"#,
        &json!([12.0, 24.0]),
    );
}
