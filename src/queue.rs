//! Queue files, `qmgr0.dat` and `qmgr1.dat` (Windows XP to 8.1; BITS
//! writes one, then the other):
//!
//! ```text
//! 0   a GUID (the same in every queue file seen)
//! 16  QUEUE_FILE marker
//! 32  JOB_LIST marker, job count, the jobs, JOB_LIST marker  (a list)
//!     … more lists (a second one, empty in the samples)
//!     QUEUE_FILE marker, the GUID of offset 0
//! ```
//!
//! The jobs of a list are separated by a marker of the BITS version, which
//! is also before the first and after the last. What follows the
//! structure is free space: older versions of it, often whole jobs.

use common::bytes::Reader;

use crate::marker;
use crate::record::{self, FileList, Job, Origin};

/// Offset of the queue file marker.
const QUEUE_FILE_AT: usize = 16;
/// Offset of the first list.
const FIRST_LIST_AT: usize = QUEUE_FILE_AT + marker::SIZE;

/// The lists of a queue file.
pub(crate) struct Lists {
    /// Their jobs, in file order.
    pub(crate) jobs: Vec<Job>,
    /// Where the structure ends: the rest is free space.
    pub(crate) end: usize,
    pub(crate) problems: Vec<String>,
}

/// Whether `data` starts like a queue file.
pub(crate) fn is_queue(data: &[u8]) -> bool {
    marker::is_at(data, QUEUE_FILE_AT, &marker::QUEUE_FILE)
}

/// The jobs of a queue file's lists.
pub(crate) fn read(data: &[u8]) -> Lists {
    let mut lists = Lists {
        jobs: Vec::new(),
        end: FIRST_LIST_AT,
        problems: Vec::new(),
    };
    let mut at = FIRST_LIST_AT;
    loop {
        if marker::is_at(data, at, &marker::QUEUE_FILE) {
            at += marker::SIZE;
            if data.get(at..at + marker::SIZE) == data.get(..marker::SIZE) {
                at += marker::SIZE;
            }
            break;
        }
        if !marker::is_at(data, at, &marker::JOB_LIST) {
            lists.problems.push(format!(
                "offset {at}: neither a job list nor the end of the lists"
            ));
            break;
        }
        match list(data, at + marker::SIZE, &mut lists) {
            Some(end) => at = end,
            None => break,
        }
    }
    lists.end = at;
    lists
}

/// The list whose job count is at `at`; the offset after it, `None` when
/// it can't be followed to its end.
fn list(data: &[u8], at: usize, lists: &mut Lists) -> Option<usize> {
    let mut reader = Reader::new(data);
    let Some(count) = reader.seek(at).ok().and_then(|()| reader.u32_le().ok()) else {
        lists
            .problems
            .push(format!("offset {at}: job count cut off"));
        return None;
    };
    let mut at = at + 4;
    let separator = data
        .get(at..at + marker::SIZE)
        .and_then(|bytes| marker::LEGACY_JOBS.iter().find(|m| m[..] == *bytes));
    if let Some(separator) = separator {
        at = jobs(data, at, count, separator, lists)?;
    } else if count > 0 {
        lists.problems.push(format!(
            "offset {at}: {count} jobs listed, but no job marker BITS writes"
        ));
        return None;
    }
    if !marker::is_at(data, at, &marker::JOB_LIST) {
        lists.problems.push(format!(
            "offset {at}: the list's jobs don't end at its end marker"
        ));
        return None;
    }
    Some(at + marker::SIZE)
}

/// `count` jobs separated by `separator` from `at`; the offset after the
/// last separator.
fn jobs(
    data: &[u8],
    mut at: usize,
    count: u32,
    separator: &[u8; marker::SIZE],
    lists: &mut Lists,
) -> Option<usize> {
    for _ in 0..count {
        at = past_separators(data, at, separator);
        let Some(end) = marker::find_any(data, at, std::slice::from_ref(separator)) else {
            lists
                .problems
                .push(format!("job at offset {at}: no marker after it"));
            return None;
        };
        match record::job(
            &data[at..end],
            Origin::Listed { offset: at },
            FileList::Records,
        ) {
            Ok(read) => {
                lists.problems.extend(
                    read.problems
                        .into_iter()
                        .map(|problem| format!("job at offset {at}: {problem}")),
                );
                lists.jobs.push(read.job);
            }
            Err(problem) => lists
                .problems
                .push(format!("job at offset {at}: {problem}")),
        }
        at = end;
    }
    Some(past_separators(data, at, separator))
}

/// The offset after the run of `separator` at `at`.
fn past_separators(data: &[u8], mut at: usize, separator: &[u8; marker::SIZE]) -> usize {
    while marker::is_at(data, at, separator) {
        at += marker::SIZE;
    }
    at
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::record::tests::{file_record, job_record};

    /// The GUID queue files start and end with.
    pub(crate) const SIGNATURE: [u8; 16] = [
        0xF5, 0x6A, 0x19, 0x2B, 0x7C, 0x00, 0x8F, 0x43, 0x8D, 0x12, 0x1C, 0xFC, 0xA4, 0xCC, 0x9B,
        0x76,
    ];

    /// A queue file listing `jobs`, then `free` bytes of free space.
    pub(crate) fn queue_file(jobs: &[Vec<u8>], free: &[u8]) -> Vec<u8> {
        let separator = marker::LEGACY_JOBS[0];
        let mut data = SIGNATURE.to_vec();
        data.extend(marker::QUEUE_FILE);
        data.extend(marker::JOB_LIST);
        data.extend((jobs.len() as u32).to_le_bytes());
        for job in jobs {
            data.extend(separator);
            data.extend(job);
        }
        if !jobs.is_empty() {
            data.extend(separator);
        }
        data.extend(marker::JOB_LIST);
        data.extend(marker::JOB_LIST);
        data.extend(0u32.to_le_bytes());
        data.extend(marker::JOB_LIST);
        data.extend(marker::QUEUE_FILE);
        data.extend(SIGNATURE);
        data.extend(free);
        data
    }

    #[test]
    fn reads_every_list() {
        let jobs = [
            job_record(&[file_record("http://a", 1)]),
            job_record(&[file_record("http://b", 2), file_record("http://c", 3)]),
        ];
        let data = queue_file(&jobs, &[0; 10]);
        let lists = read(&data);
        assert_eq!(lists.problems, Vec::<String>::new());
        assert_eq!(lists.jobs.len(), 2);
        assert_eq!(lists.jobs[1].files.len(), 2);
        assert_eq!(lists.end, data.len() - 10);
        assert_eq!(
            lists.jobs[0].origin,
            Origin::Listed {
                offset: 32 + 20 + 16
            }
        );
    }

    #[test]
    fn an_empty_queue() {
        let data = queue_file(&[], &[]);
        let lists = read(&data);
        assert!(lists.jobs.is_empty() && lists.problems.is_empty());
        assert_eq!(lists.end, data.len());
    }

    #[test]
    fn damage_is_reported() {
        let data = queue_file(&[job_record(&[file_record("http://a", 1)])], &[]);
        let mut wrong_count = data.clone();
        wrong_count[48] = 3;
        let lists = read(&wrong_count);
        assert_eq!(lists.jobs.len(), 1);
        assert!(!lists.problems.is_empty());
        let cut = read(&data[..60]);
        assert!(!cut.problems.is_empty());
        let mut unknown_separator = data;
        unknown_separator[52] ^= 0xFF;
        assert!(!read(&unknown_separator).problems.is_empty());
    }
}
