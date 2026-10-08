//! The GUIDs BITS writes between the parts of its queues, as stored (the
//! first three fields little-endian), and searching for them.

/// A marker's size.
pub(crate) const SIZE: usize = 16;

/// At offset 16 of a queue file (`qmgr0.dat`, `qmgr1.dat`), and again where
/// its lists end.
pub(crate) const QUEUE_FILE: [u8; SIZE] = guid("13F72BC84099124A9F1A3AAEBD894EEA");

/// Before and after each list of jobs of a queue file.
pub(crate) const JOB_LIST: [u8; SIZE] = guid("47445F00A9BDBA449851C47BB6C07ACE");

/// Before and after the files of a job, in both formats.
pub(crate) const FILES: [u8; SIZE] = guid("36DA56776F515A43ACAC44A248FFF34D");

/// Between the jobs of a queue file's list: each BITS version (XP to 8.1)
/// writes its own; Windows 7 the first.
pub(crate) const LEGACY_JOBS: [[u8; SIZE]; 5] = [
    guid("93362035A00C104A84F3B17E7B499CD7"),
    guid("101370C83653B34183E581557F361B87"),
    guid("8C93EA64030F6840B46FF97FE51D4DCD"),
    guid("B346ED3D3B10F944BC2FE8378BD31986"),
    guid("74E70C81D2BBCC489E47862E8D58F3C6"),
];

/// Around the job in a `Blob` of the `Jobs` table of `qmgr.db` (Windows 10
/// and later), one per BITS version.
pub(crate) const DATABASE_JOBS: [[u8; SIZE]; 6] = [
    guid("A15609E143AFC94292E66F9856EBA7F6"),
    guid("9F95D44C6470F24B84D7476A7E62699F"),
    guid("F11926A93203BF4C9427898818958831"),
    guid("C133BCDDFB5AAF4DB8A12268B39D01AD"),
    guid("D057568F2C013E4EAD2CF4A5D7656FAF"),
    guid("5067419457031D46A4CC5DD9990706E4"),
];

/// Around the file in a `Blob` of the `Files` table of `qmgr.db`.
pub(crate) const DATABASE_FILE: [u8; SIZE] = guid("E4CF9E5146D99743B73E268513051AB2");

/// Whether `data` has `marker` at `at`.
pub(crate) fn is_at(data: &[u8], at: usize, marker: &[u8; SIZE]) -> bool {
    data.get(at..at + SIZE) == Some(marker)
}

/// The offset of the first of `markers` in `data` at or after `from`.
pub(crate) fn find_any(data: &[u8], from: usize, markers: &[[u8; SIZE]]) -> Option<usize> {
    let haystack = data.get(from..)?;
    haystack
        .windows(SIZE)
        .position(|window| markers.iter().any(|marker| window == marker))
        .map(|at| from + at)
}

/// Every offset of any of `markers` in `data`, in order.
pub(crate) fn all(data: &[u8], markers: &[[u8; SIZE]]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = find_any(data, from, markers) {
        found.push(at);
        from = at + 1;
    }
    found
}

/// The 16 bytes written as 32 hexadecimal digits.
const fn guid(hex: &str) -> [u8; SIZE] {
    let digits = hex.as_bytes();
    assert!(
        digits.len() == 2 * SIZE,
        "a marker is 32 hexadecimal digits"
    );
    let mut bytes = [0; SIZE];
    let mut at = 0;
    while at < SIZE {
        bytes[at] = nibble(digits[2 * at]) << 4 | nibble(digits[2 * at + 1]);
        at += 1;
    }
    bytes
}

const fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        b'A'..=b'F' => digit - b'A' + 10,
        _ => panic!("not a hexadecimal digit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_stored_as_written() {
        assert_eq!(FILES[..4], [0x36, 0xDA, 0x56, 0x77]);
        assert_eq!(DATABASE_FILE[15], 0xB2);
    }

    #[test]
    fn searching() {
        let mut data = vec![0; 40];
        data[3..19].copy_from_slice(&FILES);
        data[20..36].copy_from_slice(&JOB_LIST);
        assert!(is_at(&data, 3, &FILES));
        assert!(!is_at(&data, 30, &FILES));
        assert_eq!(find_any(&data, 0, &[JOB_LIST, FILES]), Some(3));
        assert_eq!(find_any(&data, 4, &[JOB_LIST, FILES]), Some(20));
        assert_eq!(find_any(&data, 41, &[FILES]), None);
        assert_eq!(all(&data, &[FILES, JOB_LIST]), [3, 20]);
    }
}
