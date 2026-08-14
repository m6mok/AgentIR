//! One-call local authoring CLI for all three model payload families.

use agentir_authoring::{
    AuthoringFrame, AuthoringGateway, AuthoringSurface, ExecutionMode, parse_authoring_payload,
    parse_framed_staged, parse_task,
};
use serde_json::{Value, json};
use std::io::{self, Read};

struct Arguments {
    task_path: String,
    surface: Option<AuthoringSurface>,
    framed_staged_v2: bool,
    frame_path: Option<String>,
}

fn parse_arguments() -> Result<Arguments, Box<dyn std::error::Error>> {
    let mut values = std::env::args().skip(1);
    let mut task_path = None;
    let mut surface = None;
    let mut framed_staged_v2 = false;
    let mut frame_path = None;
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--task" if task_path.is_none() => {
                task_path = Some(values.next().ok_or("--task requires a path")?);
            }
            "--surface" if surface.is_none() => {
                let value = values.next().ok_or("--surface requires a value")?;
                surface = Some(match value.as_str() {
                    "auto" => None,
                    "graph" => Some(AuthoringSurface::Graph),
                    "incremental" | "incremental-batch" => Some(AuthoringSurface::IncrementalBatch),
                    "staged" => Some(AuthoringSurface::Staged),
                    "framed-staged-v2" => {
                        framed_staged_v2 = true;
                        None
                    }
                    _ => {
                        return Err(
                            "--surface must be auto, graph, incremental-batch, staged, or framed-staged-v2".into(),
                        );
                    }
                });
            }
            "--frame" if frame_path.is_none() => {
                frame_path = Some(values.next().ok_or("--frame requires a path")?);
            }
            _ => {
                return Err("usage: agentir-authoring --task <server-task.json> [--surface auto|graph|incremental-batch|staged|framed-staged-v2] [--frame <public-frame.json>] < payload.json".into());
            }
        }
    }
    if framed_staged_v2 != frame_path.is_some() {
        return Err("--surface framed-staged-v2 and --frame must be supplied together".into());
    }
    Ok(Arguments {
        task_path: task_path.ok_or("--task is required")?,
        surface: surface.flatten(),
        framed_staged_v2,
        frame_path,
    })
}

fn run() -> Result<Value, Box<dyn std::error::Error>> {
    let arguments = parse_arguments()?;
    let task_text = std::fs::read_to_string(arguments.task_path)?;
    let mut payload_text = String::new();
    io::stdin().read_to_string(&mut payload_text)?;
    let task = parse_task(&task_text)?;
    if arguments.framed_staged_v2 {
        let frame_text =
            std::fs::read_to_string(arguments.frame_path.expect("checked frame path"))?;
        let frame: AuthoringFrame = serde_json::from_str(&frame_text)?;
        let payload = parse_framed_staged(&payload_text)?;
        let result = AuthoringGateway::new().publish_framed_staged(
            &task,
            &frame,
            &payload,
            ExecutionMode::Native,
        )?;
        return Ok(json!({"ok":true,"result":result}));
    }
    let payload = parse_authoring_payload(&payload_text, arguments.surface)?;
    let result = AuthoringGateway::new().publish_payload(&task, &payload, ExecutionMode::Native)?;
    Ok(json!({"ok":true,"result":result}))
}

fn main() {
    if std::env::args().nth(1).as_deref()
        == Some(agentir_runtime_native_cpu::HIDDEN_WORKER_ARGUMENT)
    {
        agentir_native_cpu_worker::run_worker_stdio();
        return;
    }
    let envelope = match run() {
        Ok(success) => success,
        Err(error) => {
            let diagnostic = error
                .downcast_ref::<agentir_authoring::AuthoringError>()
                .map_or_else(
                    || {
                        json!({
                            "code":"AUTHORING_IO",
                            "path":"$",
                            "expected":"successful authoring call",
                            "actual":error.to_string(),
                        })
                    },
                    |authoring| {
                        serde_json::to_value(authoring)
                            .unwrap_or_else(|_| json!({"code":"AUTHORING_SERIALIZATION"}))
                    },
                );
            json!({"ok":false,"error":diagnostic})
        }
    };
    println!("{envelope}");
}
