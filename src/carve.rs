//! Jobs and files recovered from the bytes no live record holds: deleted
//! jobs, and older versions of jobs BITS rewrote elsewhere.
//!
//! - Whole jobs: after each job marker, a job record that reads to its end,
//!   with a type, priority and state BITS defines and plausible times.
//! - Jobs whose start was overwritten: each files marker followed by
//!   plausible times is the end of a job's files. When the previous files
//!   marker opens a list of files that ends exactly there, the files are
//!   whole, and the fields before them are read backwards: the owner's SID
//!   (the last `S-1-…` string before the files), then the notify
//!   arguments, notify program, description and name, then type, priority,
//!   state and id, as far as they are intact. Otherwise the files are read
//!   backwards from the end, each until one is cut off (the end of its
//!   temporary path kept).
//! - Files (`qmgr.db`): after each file marker, a file record; its GUID
//!   when the database record around it is still there.

use std::ops::Range;

use common::bytes::Reader;
use common::win::guid_to_string;

use crate::marker;
use crate::record::{self, Control, File, FileList, Files, Job, Origin};
use crate::text;

/// Bytes of a queue file's file record after the volume's name.
const QUEUE_FILE_AFTER_VOLUME: usize = record::FILE_TAIL + record::QUEUE_FILE_TAIL;
/// Bytes of a job's type, priority, state, 4 bytes and id.
const CONTROL_HEAD: usize = 32;
/// Bytes before a job's files searched for its owner.
const OWNER_SEARCH: usize = 16 * 1024;
/// The start of an owner's SID, `S-1-`, in UTF-16.
const SID_START: [u8; 8] = [b'S', 0, b'-', 0, b'1', 0, b'-', 0];
/// The header of an ESE record of a `Files` row, before its `Id`: last
/// fixed column 1, no variable column, variable data at offset 21.
const FILES_ROW_HEADER: [u8; 4] = [0x01, 0x7F, 0x15, 0x00];
/// Bytes from a `Files` row's `Id` to its blob's marker.
const ID_TO_BLOB: usize = 6;
/// The most files read backwards from one job.
const MAX_FILES_BACKWARDS: usize = 4096;

/// What carving found.
#[derive(Debug, Default)]
pub(crate) struct Carved {
    pub(crate) jobs: Vec<Job>,
    pub(crate) files: Vec<File>,
}

/// How the format separates its jobs.
pub(crate) struct Layout {
    pub(crate) job_markers: &'static [[u8; marker::SIZE]],
    pub(crate) list: FileList,
}

/// The jobs and files recovered from `data` outside `live`, the bytes of
/// the records the queue lists (empty for a database, whose live records
/// are left out by comparison instead).
pub(crate) fn carve(data: &[u8], layout: &Layout, live: Range<usize>) -> Carved {
    let mut claimed = vec![live];
    let mut carved = Carved::default();
    whole_jobs(data, layout, &mut claimed, &mut carved);
    partial_jobs(data, layout, &mut claimed, &mut carved);
    if layout.list == FileList::Ids {
        carved.files = database_files(data, &claimed);
    }
    carved
}

fn inside(claimed: &[Range<usize>], at: usize) -> bool {
    claimed.iter().any(|range| range.contains(&at))
}

/// Whole job records after job markers.
fn whole_jobs(data: &[u8], layout: &Layout, claimed: &mut Vec<Range<usize>>, carved: &mut Carved) {
    let markers = marker::all(data, layout.job_markers);
    for (index, &at) in markers.iter().enumerate() {
        let start = at + marker::SIZE;
        if inside(claimed, start) {
            continue;
        }
        let end = markers.get(index + 1).copied().unwrap_or(data.len());
        let Some(body) = data.get(start..end) else {
            continue;
        };
        let Ok(read) = record::job(body, Origin::Carved { offset: start }, layout.list) else {
            continue;
        };
        if read.job.is_plausible() {
            claimed.push(start..start + read.end);
            carved.jobs.push(read.job);
        }
    }
}

/// Jobs whose start is lost, found by the end of their files.
fn partial_jobs(
    data: &[u8],
    layout: &Layout,
    claimed: &mut Vec<Range<usize>>,
    carved: &mut Carved,
) {
    let anchors: Vec<usize> = marker::all(data, &[marker::FILES])
        .into_iter()
        .filter(|&at| !inside(claimed, at))
        .collect();
    let mut used = vec![false; anchors.len()];
    for index in 0..anchors.len() {
        if used[index] {
            continue;
        }
        let files_end = anchors[index];
        let mut reader = Reader::new(data);
        let Some(metadata) = reader
            .seek(files_end + marker::SIZE)
            .ok()
            .and_then(|()| record::metadata(&mut reader))
            .filter(record::Metadata::is_plausible)
        else {
            continue;
        };
        let end = reader.position();
        used[index] = true;
        let opening = (0..index).rev().find(|&before| !used[before]);
        let whole = opening.and_then(|before| {
            whole_files(data, anchors[before], files_end, layout.list).map(|f| (before, f))
        });
        let (start, control, files) = if let Some((before, files)) = whole {
            used[before] = true;
            let files_start = anchors[before];
            let low = claimed_before(claimed, files_start);
            let (start, control) = control_backwards(data, files_start, low);
            (start, control, files)
        } else {
            let low = claimed_before(claimed, files_end);
            let (start, files) = files_backwards(data, files_end, low, layout.list);
            let files = Files {
                records: files,
                ..Files::default()
            };
            (start, Control::default(), files)
        };
        if control == Control::default() && files.is_empty() {
            continue;
        }
        claimed.push(start..end);
        carved.jobs.push(Job::assemble(
            Origin::Carved { offset: start },
            control,
            files,
            metadata,
        ));
    }
}

/// The end of the claimed range nearest before `at`, or 0.
fn claimed_before(claimed: &[Range<usize>], at: usize) -> usize {
    claimed
        .iter()
        .map(|range| range.end)
        .filter(|&end| end <= at)
        .max()
        .unwrap_or(0)
}

/// The files between the markers at `start` and `end`, when they are whole
/// and end exactly at `end`.
fn whole_files(data: &[u8], start: usize, end: usize, list: FileList) -> Option<Files> {
    let mut problems = Vec::new();
    let (files, files_end) = record::files(
        data,
        start,
        Origin::Carved { offset: 0 },
        list,
        &mut problems,
    )
    .ok()?;
    (files_end == end && problems.is_empty()).then_some(files)
}

/// The fields before the files whose marker is at `files_start`, read
/// backwards no further than `low`, and where the earliest read starts.
fn control_backwards(data: &[u8], files_start: usize, low: usize) -> (usize, Control) {
    let mut control = Control::default();
    let Some((owner_at, owner, flags)) = owner_before(data, files_start, low) else {
        return (files_start, control);
    };
    control.owner = Some(owner);
    control.notify_flags = Some(flags);
    let mut start = owner_at;
    let fields: [&mut Option<String>; 4] = [
        &mut control.notify_arguments,
        &mut control.notify_program,
        &mut control.description,
        &mut control.name,
    ];
    let mut all_read = true;
    for field in fields {
        let Some((at, value)) = text::ending_at(data, low..start) else {
            all_read = false;
            break;
        };
        *field = Some(value);
        start = at;
    }
    if all_read && start >= low + CONTROL_HEAD {
        if let Some(head) = control_head(data, start - CONTROL_HEAD) {
            control.id = head.id;
            control.kind = head.kind;
            control.priority = head.priority;
            control.state = head.state;
            start -= CONTROL_HEAD;
        }
    }
    (start, control)
}

/// The type, priority, state and id at `at`, when BITS defines them.
fn control_head(data: &[u8], at: usize) -> Option<Control> {
    let mut reader = Reader::new(data);
    reader.seek(at).ok()?;
    record::control(&mut reader).filter(Control::is_plausible)
}

/// The last owner SID string before `files_start` (no further back than
/// `low`), where its count starts, and the notify flags after it.
fn owner_before(data: &[u8], files_start: usize, low: usize) -> Option<(usize, String, u32)> {
    let from = low.max(files_start.saturating_sub(OWNER_SEARCH));
    let window = data.get(from..files_start)?;
    window
        .windows(SID_START.len())
        .enumerate()
        .rev()
        .filter(|(_, bytes)| *bytes == SID_START)
        .find_map(|(offset, _)| {
            let count_at = (from + offset).checked_sub(text::COUNT_SIZE)?;
            if count_at < low {
                return None;
            }
            let mut reader = Reader::new(data);
            reader.seek(count_at).ok()?;
            let owner = text::read(&mut reader)?;
            let flags = reader.u32_le().ok()?;
            (reader.position() <= files_start).then_some((count_at, owner, flags))
        })
}

/// Files read backwards from the files marker at `end`, no further than
/// `low`, and where the earliest starts.
fn files_backwards(data: &[u8], end: usize, low: usize, list: FileList) -> (usize, Vec<File>) {
    match list {
        FileList::Records => records_backwards(data, end, low),
        FileList::Ids => (end, Vec::new()),
    }
}

/// Queue file records backwards from `end`.
fn records_backwards(data: &[u8], end: usize, low: usize) -> (usize, Vec<File>) {
    let mut files = Vec::new();
    let mut cursor = end;
    while files.len() < MAX_FILES_BACKWARDS {
        let Some((start, file)) = record_ending_at(data, cursor, low) else {
            break;
        };
        let damaged = file.damaged;
        files.push(file);
        cursor = start;
        if damaged {
            break;
        }
    }
    files.reverse();
    (cursor, files)
}

/// The queue file record ending at `end`, and where it starts (where what
/// was read of it starts, when damaged).
fn record_ending_at(data: &[u8], end: usize, low: usize) -> Option<(usize, File)> {
    let volume_end = end.checked_sub(QUEUE_FILE_AFTER_VOLUME)?;
    let (volume_at, _) = text::ending_at(data, low..volume_end)?;
    let (drive_at, _) = text::ending_at(data, low..volume_at)?;
    let sizes_at = drive_at
        .checked_sub(record::FILE_SIZES)
        .filter(|&at| at >= low)?;
    let mut reader = Reader::new(data);
    reader.seek(sizes_at).ok()?;
    let (mut file, _) = record::file_after_paths(&mut reader, Origin::Carved { offset: sizes_at })?;
    let mut start = sizes_at;
    let paths: [&mut Option<String>; 3] = [
        &mut file.temp_path,
        &mut file.remote_url,
        &mut file.local_path,
    ];
    for path in paths {
        if let Some((at, value)) = text::ending_at(data, low..start) {
            *path = Some(value);
            start = at;
        } else {
            file.damaged = true;
            break;
        }
    }
    if file.damaged && file.temp_path.is_none() {
        file.temp_path = text::tail_ending_at(data, low..start);
    }
    file.origin = Origin::Carved { offset: start };
    Some((start, file))
}

/// The file records after file markers (`qmgr.db`), outside `claimed`.
fn database_files(data: &[u8], claimed: &[Range<usize>]) -> Vec<File> {
    marker::all(data, &[marker::DATABASE_FILE])
        .into_iter()
        .filter(|&at| !inside(claimed, at))
        .filter_map(|at| {
            let start = at + marker::SIZE;
            let (mut file, _) = record::file(data, start, Origin::Carved { offset: start })?;
            file.id = row_id(data, at);
            Some(file)
        })
        .collect()
}

/// The `Id` of the `Files` row whose blob's marker is at `marker_at`, when
/// the record's header is still in front of it.
fn row_id(data: &[u8], marker_at: usize) -> Option<String> {
    let id_at = marker_at.checked_sub(ID_TO_BLOB + record::GUID_SIZE)?;
    let header_at = id_at.checked_sub(FILES_ROW_HEADER.len())?;
    (data.get(header_at..id_at)? == FILES_ROW_HEADER)
        .then(|| data[id_at..id_at + record::GUID_SIZE].try_into().ok())
        .flatten()
        .map(|bytes| guid_to_string(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::queue_file;
    use crate::record::tests::{file_record, job_record, stored};

    const QUEUE: Layout = Layout {
        job_markers: &marker::LEGACY_JOBS,
        list: FileList::Records,
    };

    #[test]
    fn whole_deleted_jobs() {
        let mut free = marker::LEGACY_JOBS[0].to_vec();
        free.extend(job_record(&[file_record("http://deleted", 7)]));
        free.extend(marker::LEGACY_JOBS[0]);
        let data = queue_file(&[], &free);
        let live_end = data.len() - free.len();
        let carved = carve(&data, &QUEUE, 0..live_end);
        assert_eq!(carved.jobs.len(), 1);
        let job = &carved.jobs[0];
        assert_eq!(
            job.origin,
            Origin::Carved {
                offset: live_end + 16
            }
        );
        assert_eq!(job.files[0].remote_url.as_deref(), Some("http://deleted"));
        assert_eq!(job.name.as_deref(), Some("Setup"));
    }

    #[test]
    fn jobs_whose_start_is_overwritten() {
        let job = job_record(&[file_record("http://a", 1)]);
        // The first 20 bytes (type, priority, state and half the id) lost.
        let mut free = vec![0xEE; 20];
        free.extend(&job[20..]);
        let data = queue_file(&[], &free);
        let carved = carve(&data, &QUEUE, 0..data.len() - free.len());
        assert_eq!(carved.jobs.len(), 1);
        let job = &carved.jobs[0];
        assert_eq!(job.id, None);
        assert_eq!(job.name.as_deref(), Some("Setup"));
        assert_eq!(job.notify_program.as_deref(), Some("C:\\evil.exe"));
        assert_eq!(job.owner.as_deref(), Some("S-1-5-18"));
        assert_eq!(job.files.len(), 1);
        assert_eq!(job.retry_delay, 600);
    }

    #[test]
    fn files_whose_list_is_overwritten() {
        let first = file_record("http://first", 1);
        let job = job_record(&[first.clone(), file_record("http://second", 2)]);
        let first_at = job.windows(16).position(|w| w == marker::FILES).unwrap() + 20;
        // Everything up to the middle of the first file's temporary path.
        let after_temp = record::FILE_SIZES
            + stored("C:\\").len()
            + stored("\\\\?\\Volume{1}\\").len()
            + QUEUE_FILE_AFTER_VOLUME;
        let cut = first_at + first.len() - after_temp - 20;
        let mut free = vec![0x11; 2];
        free.extend(&job[cut..]);
        let data = queue_file(&[], &free);
        let carved = carve(&data, &QUEUE, 0..data.len() - free.len());
        assert_eq!(carved.jobs.len(), 1);
        let files = &carved.jobs[0].files;
        assert_eq!(files.len(), 2);
        assert!(files[0].damaged);
        assert_eq!(files[0].remote_url, None);
        assert!(files[0].temp_path.as_deref().unwrap().ends_with("BIT1.tmp"));
        assert_eq!(files[0].bytes_total, Some(1));
        assert!(!files[1].damaged);
        assert_eq!(files[1].remote_url.as_deref(), Some("http://second"));
    }

    #[test]
    fn noise_is_not_a_job() {
        let mut free = marker::LEGACY_JOBS[0].to_vec();
        free.extend([0xAB; 300]);
        free.extend(marker::FILES);
        free.extend([0xCD; 200]);
        let data = queue_file(&[], &free);
        let carved = carve(&data, &QUEUE, 0..0);
        assert!(carved.jobs.is_empty());
    }

    #[test]
    fn database_file_ids() {
        let mut data = vec![0x01, 0x7F, 0x15, 0x00];
        data.extend([0x42; 16]);
        data.extend([0xFE, 0, 0, 1, 4, 0]);
        data.extend(marker::DATABASE_FILE);
        data.extend(file_record("http://f", 3));
        let files = database_files(&data, &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].id.as_deref(),
            Some("42424242-4242-4242-4242-424242424242")
        );
        data[0] = 0;
        assert_eq!(database_files(&data, &[])[0].id, None);
    }
}
