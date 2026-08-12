//! Dedicated one-request Stage 9 native CPU worker process.

#![forbid(unsafe_code)]

fn main() {
    agentir_native_cpu_worker::run_worker_stdio();
}
