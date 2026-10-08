//! The fixtures, decompressed.

use std::io::Read;

/// The fixture at `tests/fixtures/<path>.gz`, decompressed.
pub fn fixture(path: &str) -> Vec<u8> {
    let full = format!("{}/tests/fixtures/{path}.gz", env!("CARGO_MANIFEST_DIR"));
    let compressed = std::fs::File::open(&full).unwrap_or_else(|e| panic!("{full}: {e}"));
    let mut data = Vec::new();
    common::gzip::Decoder::new(compressed)
        .read_to_end(&mut data)
        .unwrap_or_else(|e| panic!("{full}: {e}"));
    data
}

/// A time as BitsParser prints it: to the second, `Z`.
pub fn seconds(time: Option<common::time::Ts>) -> String {
    time.and_then(|t| t.to_iso8601())
        .map(|iso| format!("{}Z", &iso[..19]))
        .unwrap_or_default()
}
