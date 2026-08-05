use agentir_protocol::Engine;
use serde_json::Value;

#[test]
fn one_structured_response_is_emitted_per_line() {
    let mut engine = Engine::new();
    let open =
        engine.process_line(r#"{"command":"workspace.open","request_id":"one","workspace":"w1"}"#);
    let parsed: Value = serde_json::from_str(&open).expect("response JSON");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["request_id"], "one");

    let invalid = engine.process_line("not json");
    let parsed: Value = serde_json::from_str(&invalid).expect("error response JSON");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn saxpy_example_runs_through_protocol() {
    let mut engine = Engine::new();
    let lines = include_str!("../../../examples/saxpy.jsonl")
        .lines()
        .filter(|line| !line.is_empty());
    let responses = lines
        .map(|line| engine.process_line(line))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 5);
    for response in &responses {
        let parsed: Value = serde_json::from_str(response).expect("response JSON");
        assert_eq!(parsed["ok"], true, "{response}");
    }
    let result: Value = serde_json::from_str(&responses[4]).expect("evaluation response");
    assert_eq!(
        result["result"]["outputs"]["out"],
        serde_json::json!([12.0, 24.0, 36.0, 48.0])
    );
}
