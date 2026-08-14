//! Reproducible large-scale evaluation harness for the bounded authoring SDK.

#![forbid(unsafe_code)]

#[path = "../eval_harness/mod.rs"]
mod eval_harness;

fn main() {
    if std::env::args().nth(1).as_deref()
        == Some(agentir_runtime_native_cpu::HIDDEN_WORKER_ARGUMENT)
    {
        agentir_native_cpu_worker::run_worker_stdio();
        return;
    }
    if let Err(error) = eval_harness::run_cli() {
        eprintln!("agentir-authoring-eval: {error}");
        std::process::exit(1);
    }
}
