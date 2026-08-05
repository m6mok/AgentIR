use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn oversized_line_emits_once_and_next_request_is_processed() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentir"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("CLI starts");
    let mut input = Vec::new();
    input.extend_from_slice(
        format!(
            "{{\"command\":\"workspace.open\",\"request_id\":\"too-large\",\"padding\":\"{}\"}}\n",
            "x".repeat(1024 * 1024)
        )
        .as_bytes(),
    );
    input.extend_from_slice(
        b"{\"command\":\"workspace.open\",\"request_id\":\"next\",\"workspace\":\"next\"}",
    );
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&input)
        .expect("write requests");
    let output = child.wait_with_output().expect("CLI exits");
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout).unwrap();
    let responses = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["request_id"], "too-large");
    assert_eq!(responses[0]["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(responses[1]["request_id"], "next");
    assert_eq!(responses[1]["ok"], true);
}

#[test]
fn invalid_utf8_line_does_not_desynchronize_stream() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentir"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("CLI starts");
    let mut input = b"{\"request_id\":\"bad-utf8\",\"x\":\"".to_vec();
    input.push(0xff);
    input.extend_from_slice(b"\"}\n{\"command\":\"workspace.open\",\"request_id\":\"ok\"}\n");
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], "INVALID_REQUEST");
    assert_eq!(responses[1]["ok"], true);
}
