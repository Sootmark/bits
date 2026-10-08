//! Damage goes to `problems`, never a panic: arbitrary bytes, and the
//! samples cut anywhere or with bytes changed.

mod support;

use proptest::prelude::*;
use support::{fixture, seconds};

/// Bytes of a queue file's start: its markers, then a list.
const QUEUE_HEAD: usize = 0x40;

fn read_never_panics(data: &[u8]) {
    if let Ok(queue) = bits::read(data) {
        let jobs = queue.jobs.len() + queue.carved_jobs.len();
        assert!(jobs <= data.len() / 16, "more jobs than the input can hold");
        for job in queue.jobs.iter().chain(&queue.carved_jobs) {
            // Times read from damaged records still render.
            let _ = (seconds(job.created), seconds(job.expires));
        }
    }
}

/// `data` with `changes` (offset, byte) applied, offsets wrapped to its
/// length.
fn damaged(mut data: Vec<u8>, changes: &[(usize, u8)]) -> Vec<u8> {
    for &(at, byte) in changes {
        let len = data.len();
        data[at % len] = byte;
    }
    data
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bytes(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        read_never_panics(&data);
    }

    #[test]
    fn arbitrary_bytes_behind_a_queue_file_head(tail in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut data = fixture("cfreds/qmgr1.dat")[..QUEUE_HEAD].to_vec();
        data.extend(tail);
        read_never_panics(&data);
    }

    #[test]
    fn queue_files_damaged(
        changes in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..64),
        cut in any::<usize>(),
        which in 0..2usize,
    ) {
        let name = ["cfreds/qmgr0.dat", "cfreds/qmgr1.dat"][which];
        // The records are in the first 8 KiB; the rest is zeros.
        let mut data = fixture(name);
        data.truncate(8192);
        let data = damaged(data, &changes);
        read_never_panics(&data[..cut % (data.len() + 1)]);
        read_never_panics(&data);
    }
}

proptest! {
    // A database read takes longer: fewer cases.
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn databases_damaged(
        changes in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..256),
        cut in any::<usize>(),
    ) {
        let data = damaged(fixture("go-ese/qmgr.db"), &changes);
        read_never_panics(&data[..cut % (data.len() + 1)]);
        read_never_panics(&data);
    }
}

#[test]
fn every_cut_of_a_queue_file() {
    let data = fixture("cfreds/qmgr1.dat");
    for cut in 0..8192 {
        read_never_panics(&data[..cut]);
    }
}
