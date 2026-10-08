//! Every record BitsParser reads from the fixtures, carving on, read the
//! same by this crate (`tests/oracle/*.tsv`, written by
//! `tests/oracle/gen.sh`).

mod support;

use bits::{File, Format, Job, JobType, Priority, Queue, State};
use support::{fixture, seconds};

/// What comparing gave.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    /// BitsParser's job records, and how many this crate has.
    jobs: (usize, usize),
    /// BitsParser's file records, and how many this crate has.
    files: (usize, usize),
    /// BitsParser's records whose type isn't one BITS defines.
    noise: usize,
}

fn kind_name(kind: Option<JobType>) -> &'static str {
    match kind {
        Some(JobType::Download) => "download",
        Some(JobType::Upload) => "upload",
        Some(JobType::UploadReply) => "upload_reply",
        _ => "",
    }
}

fn priority_name(priority: Option<Priority>) -> &'static str {
    match priority {
        Some(Priority::Foreground) => "foreground",
        Some(Priority::High) => "high",
        Some(Priority::Normal) => "normal",
        Some(Priority::Low) => "low",
        _ => "",
    }
}

fn state_name(state: Option<State>) -> &'static str {
    match state {
        Some(State::Queued) => "queued",
        Some(State::Connecting) => "connecting",
        Some(State::Transferring) => "transferring",
        Some(State::Suspended) => "suspended",
        Some(State::Error) => "error",
        Some(State::TransientError) => "transient_error",
        Some(State::Transferred) => "transferred",
        // BitsParser's spelling.
        Some(State::Acknowledged) => "acknowleged",
        Some(State::Cancelled) => "cancelled",
        _ => "",
    }
}

/// Whether each of `expected` that isn't empty equals its `actual`.
fn agrees(expected: &[&str], actual: &[String]) -> bool {
    expected
        .iter()
        .zip(actual)
        .all(|(expected, actual)| expected.is_empty() || expected == actual)
}

fn job_matches(fields: &[&str], job: &Job) -> bool {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let actual = [
        text(&job.id),
        text(&job.name),
        text(&job.description),
        kind_name(job.kind).to_owned(),
        priority_name(job.priority).to_owned(),
        state_name(job.state).to_owned(),
        text(&job.owner),
        text(&job.notify_program),
        text(&job.notify_arguments),
        seconds(job.created),
        seconds(job.modified),
    ];
    agrees(fields, &actual)
}

fn file_matches(fields: &[&str], file: &File) -> bool {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let actual = [
        text(&file.local_path),
        text(&file.remote_url),
        text(&file.temp_path),
        file.bytes_transferred.to_string(),
        file.bytes_total
            .map(|size| size.to_string())
            .unwrap_or_default(),
        file.volume.clone(),
    ];
    agrees(fields, &actual)
}

/// One of BitsParser's records: its first line's kind and fields, and its
/// files' fields.
struct Record<'a> {
    kind: &'a str,
    fields: Vec<&'a str>,
    files: Vec<Vec<&'a str>>,
}

/// The records of a TSV of `tests/oracle/`.
fn records(expected: &str) -> Vec<Record<'_>> {
    let mut records: Vec<Record<'_>> = Vec::new();
    for line in expected.lines() {
        let columns: Vec<&str> = line.split('\t').collect();
        let (kind, fields) = (columns[1], columns[2..].to_vec());
        match (kind, records.last_mut()) {
            ("file", Some(record)) => record.files.push(fields),
            ("job" | "noise" | "times", _) => records.push(Record {
                kind,
                fields,
                files: Vec::new(),
            }),
            _ => panic!("unexpected line {line}"),
        }
    }
    records
}

/// Each of BitsParser's records found among `queue`'s.
fn compare(queue: &Queue, expected: &str) -> Tally {
    let jobs: Vec<&Job> = queue.jobs.iter().chain(&queue.carved_jobs).collect();
    let every_file: Vec<&File> = jobs
        .iter()
        .flat_map(|job| &job.files)
        .chain(&queue.orphan_files)
        .chain(&queue.carved_files)
        .collect();
    let mut tally = Tally::default();
    for record in records(expected) {
        let fields = &record.fields;
        // The jobs whose files the record's must be among; `None`: any file.
        let (owners, file_time): (Option<Vec<&Job>>, &str) = match (record.kind, queue.format) {
            ("job", _) => {
                tally.jobs.0 += 1;
                let found: Vec<&Job> = jobs
                    .iter()
                    .copied()
                    .filter(|job| job_matches(fields, job))
                    .collect();
                assert!(!found.is_empty(), "no job like BitsParser's {fields:?}");
                tally.jobs.1 += 1;
                (Some(found), "")
            }
            ("noise", _) => {
                tally.noise += 1;
                (None, "")
            }
            // A file record BitsParser couldn't tie to a job: the time is
            // the file's.
            (_, Format::Database) => (None, fields[0]),
            // A job whose start is lost: the times are the job's.
            (_, Format::QueueFile) => {
                let found = jobs
                    .iter()
                    .copied()
                    .filter(|job| {
                        seconds(job.created) == fields[0] && seconds(job.modified) == fields[1]
                    })
                    .collect();
                (Some(found), "")
            }
        };
        for file_fields in &record.files {
            tally.files.0 += 1;
            let found = match &owners {
                Some(owners) => owners
                    .iter()
                    .flat_map(|job| &job.files)
                    .any(|file| file_matches(file_fields, file)),
                None => every_file.iter().any(|file| {
                    file_matches(file_fields, file)
                        && (file_time.is_empty() || seconds(file.remote_modified) == file_time)
                }),
            };
            assert!(found, "no file like BitsParser's {file_fields:?}");
            tally.files.1 += 1;
        }
    }
    tally
}

fn expected(name: &str) -> String {
    let path = format!("{}/tests/oracle/{name}.tsv", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn windows_11_database() {
    let queue = bits::read(&fixture("go-ese/qmgr.db")).unwrap();
    let tally = compare(&queue, &expected("qmgr.db"));
    assert_eq!(
        tally,
        Tally {
            jobs: (48, 48),
            files: (76, 76),
            noise: 6,
        }
    );
    // Beyond BitsParser: jobs whose marker a page header overwrote, and
    // remains of jobs that keep only their times and file ids.
    assert_eq!(queue.jobs.len(), 1);
    assert_eq!(queue.carved_jobs.len(), 55);
    assert_eq!(queue.carved_files.len(), 80);
}

#[test]
fn windows_7_queue_files() {
    for (name, listed, carved) in [("qmgr0.dat", 0, 2), ("qmgr1.dat", 1, 1)] {
        let queue = bits::read(&fixture(&format!("cfreds/{name}"))).unwrap();
        let tally = compare(&queue, &expected(name));
        assert_eq!(
            (tally.jobs.0 - tally.jobs.1, tally.files.0 - tally.files.1),
            (0, 0),
            "{name}"
        );
        assert_eq!(
            (queue.jobs.len(), queue.carved_jobs.len()),
            (listed, carved),
            "{name}"
        );
        assert!(queue.problems.is_empty(), "{name}: {:?}", queue.problems);
    }
}
