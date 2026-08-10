use agentir_protocol::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use std::{env, fs, path::PathBuf};

fn output_path() -> Result<PathBuf, String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 2 && args[0] == "--output" {
        Ok(PathBuf::from(&args[1]))
    } else {
        Err("usage: stage8a_study --output PATH".to_owned())
    }
}

fn responses(source: &str) -> Result<Vec<Value>, String> {
    let mut engine = Engine::new();
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(&engine.process_line(line)).map_err(|error| error.to_string())
        })
        .collect()
}

fn response<'a>(responses: &'a [Value], request_id: &str) -> Result<&'a Value, String> {
    responses
        .iter()
        .find(|value| value["request_id"] == request_id)
        .ok_or_else(|| format!("missing response `{request_id}`"))
}

fn write(path: PathBuf, value: &impl Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn main() -> Result<(), String> {
    let output = output_path()?;
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;

    let saxpy = responses(include_str!("../../../examples/cpu_saxpy.jsonl"))?;
    let elementwise = responses(include_str!(
        "../../../examples/cpu_scalar_elementwise.jsonl"
    ))?;
    let rejection = responses(include_str!(
        "../../../examples/cpu_rejected_reduction.jsonl"
    ))?;

    for value in &saxpy {
        if value["ok"] != true {
            return Err(format!("unexpected SAXPY rejection: {value}"));
        }
    }
    for value in &elementwise {
        if value["ok"] != true {
            return Err(format!("unexpected elementwise rejection: {value}"));
        }
    }
    let rejected = response(&rejection, "rejected-reduction")?;
    if rejected["ok"] != false || rejected["error"]["code"] != "UNSUPPORTED_CPU_LOWERING" {
        return Err(format!("unexpected reduction result: {rejected}"));
    }

    let saxpy_semantics = json!({
        "reference": response(&saxpy, "reference")?["result"],
        "emission": response(&saxpy, "emit")?["result"],
        "package": response(&saxpy, "query")?["result"],
        "check": response(&saxpy, "check")?["result"],
        "list": response(&saxpy, "list")?["result"],
        "execution": response(&saxpy, "execute")?["result"],
    });
    let elementwise_semantics = json!({
        "emission": response(&elementwise, "emit")?["result"],
        "execution": response(&elementwise, "execute")?["result"],
    });
    let rejection_semantics = json!({"error": rejected["error"]});
    let summary = json!({
        "schema_version": "agentir.stage8a.study.v1",
        "target_profile": "cpu_scalar_v1",
        "cpu_artifact_hash": saxpy_semantics["package"]["cpu_artifact_hash"],
        "saxpy_output": saxpy_semantics["execution"]["outputs"]["out"],
        "reference_equal": saxpy_semantics["execution"]["outputs"]
            == saxpy_semantics["reference"]["outputs"],
        "timing_recorded": false,
        "device_calls": 0,
    });

    write(output.join("cpu-saxpy.json"), &saxpy_semantics)?;
    write(
        output.join("cpu-scalar-elementwise.json"),
        &elementwise_semantics,
    )?;
    write(
        output.join("cpu-rejected-reduction.json"),
        &rejection_semantics,
    )?;
    write(output.join("summary.json"), &summary)?;
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|error| error.to_string())?
    );
    Ok(())
}
