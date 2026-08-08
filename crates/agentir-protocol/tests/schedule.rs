use agentir_protocol::Engine;
use serde_json::{Value, json};

#[test]
fn stage4_serial_schedule_is_end_to_end_and_hash_separated() {
    let mut engine = Engine::new();
    let responses = include_str!("../../../examples/schedule_serial.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| engine.process_line(line))
        .collect::<Vec<_>>();
    for response in &responses {
        let parsed: Value = serde_json::from_str(response).expect("response JSON");
        assert_eq!(parsed["ok"], true, "{response}");
    }
    let created: Value = serde_json::from_str(&responses[6]).unwrap();
    let query = &created["result"]["query"];
    assert_ne!(query["schedule_hash"], query["memory_hash"]);
    assert_ne!(query["schedule_hash"], query["target_hash"]);
    assert_eq!(query["status"], "resource_valid");
    let evaluated: Value = serde_json::from_str(responses.last().unwrap()).unwrap();
    assert_eq!(
        evaluated["result"]["evaluation"]["outputs"]["out"],
        json!([12.0, 24.0, 36.0, 48.0])
    );
}

#[test]
fn stage4_examples_remain_valid_jsonl_protocol_programs() {
    for source in [
        include_str!("../../../examples/schedule_tiled.jsonl"),
        include_str!("../../../examples/schedule_remainder.jsonl"),
        include_str!("../../../examples/schedule_fused.jsonl"),
        include_str!("../../../examples/schedule_vectorized.jsonl"),
        include_str!("../../../examples/schedule_guarded_memory.jsonl"),
        include_str!("../../../examples/equality_to_schedule.jsonl"),
    ] {
        let mut engine = Engine::new();
        for line in source.lines().filter(|line| !line.is_empty()) {
            let response = engine.process_line(line);
            let parsed: Value = serde_json::from_str(&response).expect("response JSON");
            assert_eq!(parsed["ok"], true, "{response}");
        }
    }
}
