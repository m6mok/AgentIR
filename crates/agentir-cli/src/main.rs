//! JSON Lines stdin/stdout frontend.

use agentir_protocol::Engine;
use std::io::{self, BufRead, Write};

fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut engine = Engine::new();
    for line in stdin.lock().lines() {
        let response = engine.process_line(&line?);
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("agentir CLI failed: {error}");
        std::process::exit(1);
    }
}
