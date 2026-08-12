#![forbid(unsafe_code)]

fn main() {
    use std::io::Write as _;
    let _ = std::io::stdout()
        .write_all(br#"{"status":"error","error":{"code":"fixture","message":"fixture"}}extra"#);
}
