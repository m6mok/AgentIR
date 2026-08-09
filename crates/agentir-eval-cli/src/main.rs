//! JSON Lines frontend for the Stage 6A policy evaluation harness.

use agentir_policy_eval::EvaluationProtocol;
use std::io::{self, BufRead, Write};

fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut protocol = EvaluationProtocol::new().map_err(|error| {
        io::Error::other(format!("evaluation initialization failed: {error:?}"))
    })?;
    for line in stdin.lock().lines() {
        let line = line?;
        let response = protocol.process_line(&line);
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("agentir-eval failed: {error}");
        std::process::exit(1);
    }
}
