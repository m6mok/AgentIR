//! JSON Lines stdin/stdout frontend.

use agentir_protocol::Engine;
use std::io::{self, BufRead, Write};

struct BoundedLine {
    retained: Vec<u8>,
    bytes: u64,
    oversized: bool,
}

fn read_bounded_line(reader: &mut impl BufRead, max_bytes: u64) -> io::Result<Option<BoundedLine>> {
    let retained_capacity = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut retained = Vec::new();
    let mut bytes = 0_u64;
    let mut saw_data = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if saw_data {
                Ok(Some(BoundedLine {
                    oversized: bytes > max_bytes,
                    retained,
                    bytes,
                }))
            } else {
                Ok(None)
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_len = newline.unwrap_or(available.len());
        saw_data = true;
        bytes = bytes.saturating_add(u64::try_from(content_len).unwrap_or(u64::MAX));
        if retained.len() < retained_capacity {
            let remaining = retained_capacity - retained.len();
            retained.extend_from_slice(&available[..content_len.min(remaining)]);
        }
        let consumed = content_len + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(BoundedLine {
                oversized: bytes > max_bytes,
                retained,
                bytes,
            }));
        }
    }
}

fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut engine = Engine::new();
    let mut reader = stdin.lock();
    while let Some(line) = read_bounded_line(&mut reader, engine.max_request_bytes())? {
        let response = if line.oversized {
            engine.oversized_line_response(&line.retained, line.bytes)
        } else {
            engine.process_bytes(&line.retained)
        };
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
