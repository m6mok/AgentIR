use sha2::{Digest, Sha256};
use std::path::Path;

#[test]
fn frozen_luna_and_terra_fixture_bytes_are_pinned() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let manifest = include_str!("fixtures/frozen-luna-terra.sha256");
    let mut checked = 0;
    for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
        let (expected, name) = line
            .split_once("  ")
            .expect("hash manifest uses two-space separator");
        let bytes = std::fs::read(root.join(name)).expect("frozen fixture remains present");
        let actual =
            Sha256::digest(bytes)
                .iter()
                .fold(String::with_capacity(64), |mut output, byte| {
                    use std::fmt::Write as _;
                    write!(output, "{byte:02x}").expect("write to string");
                    output
                });
        assert_eq!(actual, expected, "{name} changed");
        checked += 1;
    }
    assert_eq!(checked, 34);
}
