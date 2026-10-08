//! A job and its files as BITS stores them, the same in both formats:
//!
//! ```text
//! job:      type, priority, state, (4 bytes), id (GUID)
//!           name, description, notify program, notify arguments, owner SID
//!           notify flags, then the job's security (up to the files marker)
//!           FILES marker, file count, the files, FILES marker
//!           error count, the errors (25 bytes each), transient error count,
//!           retry delay, no-progress timeout (seconds),
//!           created, modified, completed (FILETIME), 8 + 6 bytes,
//!           8 bytes, expires (FILETIME), then what newer versions add
//! file:     local path, remote URL, temporary path
//!           bytes transferred, bytes total (all ones: unknown), 1 byte
//!           drive, volume
//!           4 bytes, volume serial number, size, 1 byte, 12 bytes,
//!           the remote file's time (FILETIME), then (queue files of
//!           Windows 7) 36 bytes
//! ```
//!
//! Strings are BITS's counted UTF-16 (see [`crate::text`]). In a queue file
//! the files follow their job; in `qmgr.db` the job lists their GUIDs and
//! each is a row of the `Files` table.

use common::bytes::{checked_count, Reader};
use common::time::Ts;
use common::win::guid_to_string;

use crate::marker;
use crate::text;

/// Bytes of each error a job keeps.
const ERROR_SIZE: usize = 25;
/// The most errors read from one job.
const MAX_ERRORS: u64 = 4096;
/// The most files read from one job.
const MAX_FILES: u64 = 1 << 20;
/// Bytes between the completion time and the expiry time: a time, 6 bytes
/// and another time, none of them read.
const BEFORE_EXPIRES: usize = 22;
/// Bytes of a file record's sizes and the byte after them.
pub(crate) const FILE_SIZES: usize = 8 + 8 + 1;
/// Bytes of a file record before the volume serial number, after the
/// volume.
const BEFORE_SERIAL: usize = 4;
/// Bytes of a file record's tail this crate doesn't read, between the
/// volume serial number and the remote file's time.
const BEFORE_REMOTE_TIME: usize = 21;
/// Bytes of a file record after the volume, to the end of the remote
/// file's time.
pub(crate) const FILE_TAIL: usize = BEFORE_SERIAL + 4 + BEFORE_REMOTE_TIME + 8;
/// Bytes of a queue file's file record after the remote file's time (as
/// Windows 7 writes it).
pub(crate) const QUEUE_FILE_TAIL: usize = 36;
/// Bytes of a GUID.
pub(crate) const GUID_SIZE: usize = 16;
/// `bytes total` when the size isn't known yet.
const UNKNOWN_SIZE: u64 = u64::MAX;
/// Times outside these years aren't a job's, when telling a carved job
/// from noise.
const PLAUSIBLE_YEARS: (i64, i64) = (1995, 2100);

/// Where a record was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A row of `qmgr.db`'s `Jobs` or `Files` table, on this leaf page.
    Row {
        /// The leaf page.
        page: u32,
    },
    /// A job of a queue file's list, or one of its files, at this offset.
    Listed {
        /// The record's offset in the file.
        offset: usize,
    },
    /// Recovered from the file's free space, at this offset: a job or file
    /// deleted, or an older version of one.
    Carved {
        /// The record's offset in the file.
        offset: usize,
    },
}

impl Origin {
    /// The same kind of origin, `delta` bytes further on.
    pub(crate) fn advanced(self, delta: usize) -> Self {
        match self {
            Self::Row { page } => Self::Row { page },
            Self::Listed { offset } => Self::Listed {
                offset: offset + delta,
            },
            Self::Carved { offset } => Self::Carved {
                offset: offset + delta,
            },
        }
    }
}

/// What a job transfers (`BG_JOB_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobType {
    /// Downloads files.
    Download,
    /// Uploads a file.
    Upload,
    /// Uploads a file and receives the server's reply.
    UploadReply,
    /// A value BITS doesn't define.
    Other(u32),
}

/// A job's priority (`BG_JOB_PRIORITY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Transfers in the foreground.
    Foreground,
    /// The highest background priority.
    High,
    /// The default.
    Normal,
    /// The lowest.
    Low,
    /// A value BITS doesn't define.
    Other(u32),
}

/// A job's state (`BG_JOB_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Waiting to run.
    Queued,
    /// Connecting to the server.
    Connecting,
    /// Transferring.
    Transferring,
    /// Suspended (paused).
    Suspended,
    /// Failed for good.
    Error,
    /// Failed, to be retried.
    TransientError,
    /// Every file transferred, not yet acknowledged.
    Transferred,
    /// Transferred and acknowledged by its owner: the files are in place.
    Acknowledged,
    /// Cancelled.
    Cancelled,
    /// A value BITS doesn't define.
    Other(u32),
}

impl JobType {
    fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Download,
            1 => Self::Upload,
            2 => Self::UploadReply,
            other => Self::Other(other),
        }
    }
}

impl Priority {
    fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Foreground,
            1 => Self::High,
            2 => Self::Normal,
            3 => Self::Low,
            other => Self::Other(other),
        }
    }
}

impl State {
    fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Queued,
            1 => Self::Connecting,
            2 => Self::Transferring,
            3 => Self::Suspended,
            4 => Self::Error,
            5 => Self::TransientError,
            6 => Self::Transferred,
            7 => Self::Acknowledged,
            8 => Self::Cancelled,
            other => Self::Other(other),
        }
    }
}

/// A BITS job.
///
/// A job carved from free space may have lost its start: then the fields
/// read before its files (`id` to `notify_flags`) are `None`, all or some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Where it was read.
    pub origin: Origin,
    /// Its GUID (`d387e452-e2b3-472d-b3b3-85c2be33cbb3`).
    pub id: Option<String>,
    /// What it transfers.
    pub kind: Option<JobType>,
    /// Its priority.
    pub priority: Option<Priority>,
    /// Its state.
    pub state: Option<State>,
    /// Its name, given by the program that made it.
    pub name: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// The program BITS runs when the job completes or fails
    /// (`SetNotifyCmdLine`, what `bitsadmin /SetNotifyCmdLine` sets): a way
    /// to run a program again and again, as `SYSTEM` for a system job.
    /// Empty when none is set.
    pub notify_program: Option<String>,
    /// That program's command line.
    pub notify_arguments: Option<String>,
    /// The owner's SID (`S-1-5-18` for `SYSTEM`).
    pub owner: Option<String>,
    /// When BITS notifies its owner (`BG_NOTIFY_*` flags: 1 transferred, 2
    /// error, 4 notifications disabled, 8 modification, 16 file
    /// transferred).
    pub notify_flags: Option<u32>,
    /// How many files it says it has.
    pub file_count: Option<u32>,
    /// The GUIDs of its files (`qmgr.db`: the `Id` of their `Files` rows).
    pub file_ids: Vec<String>,
    /// Its files, those that could be read.
    pub files: Vec<File>,
    /// How many errors it keeps.
    pub error_count: u32,
    /// How many transient errors it met.
    pub transient_error_count: u32,
    /// Seconds BITS waits before retrying after a transient error.
    pub retry_delay: u32,
    /// Seconds BITS keeps retrying without progress before giving up.
    pub no_progress_timeout: u32,
    /// When it was created (UTC).
    pub created: Option<Ts>,
    /// When it was last modified (UTC).
    pub modified: Option<Ts>,
    /// When its transfer completed (UTC).
    pub completed: Option<Ts>,
    /// When BITS deletes it unless it changes (UTC): 90 days after its last
    /// modification by default.
    pub expires: Option<Ts>,
}

/// A file of a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Where it was read.
    pub origin: Origin,
    /// Its GUID (`qmgr.db` only: the `Id` of its `Files` row; for a carved
    /// file, when the record around it still gives it).
    pub id: Option<String>,
    /// The URL it is downloaded from (uploaded to, for an upload job).
    pub remote_url: Option<String>,
    /// Where it goes on disk (comes from, for an upload job).
    pub local_path: Option<String>,
    /// The temporary file BITS writes while transferring (`BIT*.tmp`).
    pub temp_path: Option<String>,
    /// Whether the record's start was overwritten (carved records only):
    /// the paths read before the damage are `None`, and `temp_path` may be
    /// its end only.
    pub damaged: bool,
    /// Bytes transferred.
    pub bytes_transferred: u64,
    /// The file's size; `None` while unknown.
    pub bytes_total: Option<u64>,
    /// The drive it goes to (`C:\`).
    pub drive: String,
    /// The volume it goes to (`\\?\Volume{…}\`).
    pub volume: String,
    /// That volume's serial number (as `vol` shows it, `CA0C-7A48`).
    pub volume_serial: Option<u32>,
    /// The time BITS keeps of the remote file, to the second: its
    /// `Last-Modified` time, as far as the samples show (BitsParser reports
    /// it as a creation time).
    pub remote_modified: Option<Ts>,
}

/// How a job lists its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileList {
    /// The records themselves (queue files).
    Records,
    /// Their GUIDs (`qmgr.db`).
    Ids,
}

/// A job read, where it ends, and what was wrong in it.
#[derive(Debug)]
pub(crate) struct ReadJob {
    pub(crate) job: Job,
    /// The offset just past what was read (its fixed fields).
    pub(crate) end: usize,
    pub(crate) problems: Vec<String>,
}

/// The fields before the files of a job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Control {
    pub(crate) id: Option<String>,
    pub(crate) kind: Option<JobType>,
    pub(crate) priority: Option<Priority>,
    pub(crate) state: Option<State>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) notify_program: Option<String>,
    pub(crate) notify_arguments: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) notify_flags: Option<u32>,
}

/// The files of a job: how many it says it has, their GUIDs (`qmgr.db`)
/// and the records read (queue files).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Files {
    pub(crate) count: Option<u32>,
    pub(crate) ids: Vec<String>,
    pub(crate) records: Vec<File>,
}

impl Files {
    /// Whether nothing of the files is known.
    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.records.is_empty()
    }
}

/// The fields after the files of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub(crate) error_count: u32,
    pub(crate) transient_error_count: u32,
    pub(crate) retry_delay: u32,
    pub(crate) no_progress_timeout: u32,
    pub(crate) created: Option<Ts>,
    pub(crate) modified: Option<Ts>,
    pub(crate) completed: Option<Ts>,
    pub(crate) expires: Option<Ts>,
}

impl Metadata {
    /// Whether the times are a job's (see [`times_plausible`]).
    pub(crate) fn is_plausible(&self) -> bool {
        times_plausible(self.created, self.modified)
    }
}

/// Whether a job's times are plausible: created in a plausible year, and
/// modified then or after.
fn times_plausible(created: Option<Ts>, modified: Option<Ts>) -> bool {
    let (first, last) = PLAUSIBLE_YEARS;
    match (created, modified) {
        (Some(created), Some(modified)) => {
            created.year_within(first, last)
                && modified.year_within(first, last)
                && modified.ticks() >= created.ticks()
        }
        _ => false,
    }
}

impl Control {
    /// Whether type, priority and state are values BITS defines.
    pub(crate) fn is_plausible(&self) -> bool {
        !matches!(self.kind, None | Some(JobType::Other(_)))
            && !matches!(self.priority, None | Some(Priority::Other(_)))
            && !matches!(self.state, None | Some(State::Other(_)))
    }
}

impl Job {
    /// A job of these parts.
    pub(crate) fn assemble(
        origin: Origin,
        control: Control,
        files: Files,
        metadata: Metadata,
    ) -> Self {
        Self {
            origin,
            id: control.id,
            kind: control.kind,
            priority: control.priority,
            state: control.state,
            name: control.name,
            description: control.description,
            notify_program: control.notify_program,
            notify_arguments: control.notify_arguments,
            owner: control.owner,
            notify_flags: control.notify_flags,
            file_count: files.count,
            file_ids: files.ids,
            files: files.records,
            error_count: metadata.error_count,
            transient_error_count: metadata.transient_error_count,
            retry_delay: metadata.retry_delay,
            no_progress_timeout: metadata.no_progress_timeout,
            created: metadata.created,
            modified: metadata.modified,
            completed: metadata.completed,
            expires: metadata.expires,
        }
    }

    /// Whether it reads as a job rather than noise: a type, priority and
    /// state BITS defines, and plausible times.
    pub(crate) fn is_plausible(&self) -> bool {
        let control = Control {
            kind: self.kind,
            priority: self.priority,
            state: self.state,
            ..Control::default()
        };
        control.is_plausible() && times_plausible(self.created, self.modified)
    }

    /// Whether `other` holds the same, wherever each was read.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        let unplaced = |job: &Self| {
            let mut job = job.clone();
            job.origin = Origin::Row { page: 0 };
            for file in &mut job.files {
                file.origin = Origin::Row { page: 0 };
            }
            job
        };
        unplaced(self) == unplaced(other)
    }
}

impl File {
    /// Whether `other` holds the same, wherever each was read.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        let mut other = other.clone();
        other.origin = self.origin;
        *self == other
    }
}

/// The job whose record is `body` (its first byte the job type's), read
/// at `origin`.
///
/// # Errors
/// What made it unreadable, when a fixed field is cut off or a string
/// isn't one.
pub(crate) fn job(body: &[u8], origin: Origin, list: FileList) -> Result<ReadJob, String> {
    let mut reader = Reader::new(body);
    let control = control(&mut reader).ok_or("its type, id, names or owner don't read")?;
    let files_start = marker::find_any(body, reader.position(), &[marker::FILES])
        .ok_or("no files marker after its owner")?;
    let mut problems = Vec::new();
    let (files, files_end) = files(body, files_start, origin, list, &mut problems)?;
    let mut reader = Reader::new(body);
    reader
        .seek(files_end + marker::SIZE)
        .map_err(|e| e.to_string())?;
    let metadata = metadata(&mut reader).ok_or("its times are cut off")?;
    Ok(ReadJob {
        job: Job::assemble(origin, control, files, metadata),
        end: reader.position(),
        problems,
    })
}

/// Type, priority, state, id, names, owner and notify flags.
pub(crate) fn control(reader: &mut Reader<'_>) -> Option<Control> {
    let kind = JobType::from_u32(reader.u32_le().ok()?);
    let priority = Priority::from_u32(reader.u32_le().ok()?);
    let state = State::from_u32(reader.u32_le().ok()?);
    reader.skip(4).ok()?;
    let id = guid_to_string(&reader.array().ok()?);
    let mut next = || text::read(reader);
    let (name, description, notify_program, notify_arguments, owner) =
        (next()?, next()?, next()?, next()?, next()?);
    Some(Control {
        id: Some(id),
        kind: Some(kind),
        priority: Some(priority),
        state: Some(state),
        name: Some(name),
        description: Some(description),
        notify_program: Some(notify_program),
        notify_arguments: Some(notify_arguments),
        owner: Some(owner),
        notify_flags: Some(reader.u32_le().ok()?),
    })
}

/// The files between the marker at `start` and the next one: their count,
/// GUIDs and records, and where the closing marker is.
pub(crate) fn files(
    body: &[u8],
    start: usize,
    origin: Origin,
    list: FileList,
    problems: &mut Vec<String>,
) -> Result<(Files, usize), String> {
    let count_at = start + marker::SIZE;
    let mut reader = Reader::new(body);
    reader.seek(count_at).map_err(|e| e.to_string())?;
    let count = reader.u32_le().map_err(|e| e.to_string())?;
    let end = marker::find_any(body, reader.position(), &[marker::FILES])
        .ok_or("no marker after its files")?;
    let section = &body[..end];
    let mut ids = Vec::new();
    let mut records = Vec::new();
    match list {
        FileList::Ids => {
            let available = end - reader.position();
            let wanted = checked_count(count.into(), GUID_SIZE, MAX_FILES, available, count_at)
                .map_err(|e| e.to_string())?;
            for _ in 0..wanted {
                ids.push(guid_to_string(&reader.array().map_err(|e| e.to_string())?));
            }
            if reader.position() != end {
                problems.push(format!(
                    "{} bytes between its {count} file ids and their end marker",
                    end - reader.position()
                ));
            }
        }
        FileList::Records => {
            records = file_records(section, reader.position(), count, origin, problems);
        }
    }
    let files = Files {
        count: Some(count),
        ids,
        records,
    };
    Ok((files, end))
}

/// `count` file records from `at` to the end of `section`.
fn file_records(
    section: &[u8],
    mut at: usize,
    count: u32,
    origin: Origin,
    problems: &mut Vec<String>,
) -> Vec<File> {
    let mut records = Vec::new();
    while records.len() < count as usize && at < section.len() {
        let Some((file, end)) = file(section, at, origin.advanced(at)) else {
            problems.push(format!(
                "file {} of {count}: no record at offset {at} of the job",
                records.len() + 1
            ));
            break;
        };
        records.push(file);
        at = (end + QUEUE_FILE_TAIL).min(section.len());
    }
    if records.len() < count as usize {
        problems.push(format!("{count} files listed, {} read", records.len()));
    } else if at < section.len() {
        problems.push(format!(
            "{} bytes after its files, before their end marker",
            section.len() - at
        ));
    }
    records
}

/// The file record at `at` of `data`, and where its fields this crate
/// reads end.
pub(crate) fn file(data: &[u8], at: usize, origin: Origin) -> Option<(File, usize)> {
    let mut reader = Reader::new(data);
    reader.seek(at).ok()?;
    let local_path = text::read(&mut reader)?;
    let remote_url = text::read(&mut reader)?;
    let temp_path = text::read(&mut reader)?;
    let (file, end) = file_after_paths(&mut reader, origin)?;
    Some((
        File {
            local_path: Some(local_path),
            remote_url: Some(remote_url),
            temp_path: Some(temp_path),
            ..file
        },
        end,
    ))
}

/// A file record's fields after its paths (the paths left `None`).
pub(crate) fn file_after_paths(reader: &mut Reader<'_>, origin: Origin) -> Option<(File, usize)> {
    let bytes_transferred = reader.u64_le().ok()?;
    let bytes_total = Some(reader.u64_le().ok()?).filter(|&size| size != UNKNOWN_SIZE);
    reader.skip(FILE_SIZES - 16).ok()?;
    let drive = text::read(reader)?;
    let volume = text::read(reader)?;
    let mut file = File {
        origin,
        id: None,
        remote_url: None,
        local_path: None,
        temp_path: None,
        damaged: false,
        bytes_transferred,
        bytes_total,
        drive,
        volume,
        volume_serial: None,
        remote_modified: None,
    };
    let tail = reader.position();
    if let Some((serial, remote_modified)) = file_tail(reader) {
        file.volume_serial = Some(serial);
        file.remote_modified = remote_modified;
        Some((file, reader.position()))
    } else {
        Some((file, tail))
    }
}

/// The volume serial number and the remote file's time after the volume.
fn file_tail(reader: &mut Reader<'_>) -> Option<(u32, Option<Ts>)> {
    reader.skip(BEFORE_SERIAL).ok()?;
    let serial = reader.u32_le().ok()?;
    reader.skip(BEFORE_REMOTE_TIME).ok()?;
    Some((serial, filetime(reader.u64_le().ok()?)))
}

/// Errors, retry settings and times.
pub(crate) fn metadata(reader: &mut Reader<'_>) -> Option<Metadata> {
    let error_count = reader.u32_le().ok()?;
    let errors = checked_count(
        error_count.into(),
        ERROR_SIZE,
        MAX_ERRORS,
        reader.remaining(),
        reader.offset(),
    )
    .ok()?;
    reader.skip(errors * ERROR_SIZE).ok()?;
    let transient_error_count = reader.u32_le().ok()?;
    let retry_delay = reader.u32_le().ok()?;
    let no_progress_timeout = reader.u32_le().ok()?;
    let created = filetime(reader.u64_le().ok()?);
    let modified = filetime(reader.u64_le().ok()?);
    let completed = filetime(reader.u64_le().ok()?);
    reader.skip(BEFORE_EXPIRES).ok()?;
    let expires = filetime(reader.u64_le().ok()?);
    Some(Metadata {
        error_count,
        transient_error_count,
        retry_delay,
        no_progress_timeout,
        created,
        modified,
        completed,
        expires,
    })
}

/// A FILETIME; `None` for zero (not set).
pub(crate) fn filetime(value: u64) -> Option<Ts> {
    (value != 0).then(|| Ts::from_filetime(value))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `text` as BITS stores it.
    pub(crate) fn stored(text: &str) -> Vec<u8> {
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut bytes = (units.len() as u32).to_le_bytes().to_vec();
        bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        bytes
    }

    /// 2015-03-25 14:50:55 UTC, plus `seconds`.
    pub(crate) fn time(seconds: u64) -> u64 {
        0x01D0_670B_1912_E447 + seconds * 10_000_000
    }

    /// A Windows 7 file record.
    pub(crate) fn file_record(url: &str, size: u64) -> Vec<u8> {
        let mut bytes = stored("C:\\Users\\a\\x.exe");
        bytes.extend(stored(url));
        bytes.extend(stored("C:\\Users\\a\\BIT1.tmp"));
        bytes.extend(size.to_le_bytes());
        bytes.extend(size.to_le_bytes());
        bytes.push(0);
        bytes.extend(stored("C:\\"));
        bytes.extend(stored("\\\\?\\Volume{1}\\"));
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0xCA0C_7A48u32.to_le_bytes());
        bytes.extend(size.to_le_bytes());
        bytes.push(0x80);
        bytes.extend([0; 12]);
        bytes.extend(time(0).to_le_bytes());
        bytes.extend([0; QUEUE_FILE_TAIL]);
        bytes
    }

    /// A job's fields before its files.
    pub(crate) fn control_record(name: &str, program: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [0u32, 2, 6, 0] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend((1..=16).collect::<Vec<u8>>());
        for text in [name, "", program, "/c x", "S-1-5-18"] {
            bytes.extend(stored(text));
        }
        bytes.extend(11u32.to_le_bytes());
        bytes.extend([0x5A; 40]);
        bytes
    }

    /// A job's fields after its files.
    pub(crate) fn metadata_record() -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [0u32, 0, 600, 1_209_600] {
            bytes.extend(value.to_le_bytes());
        }
        for value in [time(0), time(5), time(5), 0] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend([0, 0, 0, 0, 0, 1]);
        bytes.extend(time(5).to_le_bytes());
        bytes.extend(time(5 + 90 * 86_400).to_le_bytes());
        bytes
    }

    /// A queue file's job with these files.
    pub(crate) fn job_record(files: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = control_record("Setup", "C:\\evil.exe");
        bytes.extend(marker::FILES);
        bytes.extend((files.len() as u32).to_le_bytes());
        for file in files {
            bytes.extend(file);
        }
        bytes.extend(marker::FILES);
        bytes.extend(metadata_record());
        bytes
    }

    #[test]
    fn reads_a_job_and_its_files() {
        let files = [file_record("http://a/1", 10), file_record("http://a/2", 20)];
        let body = job_record(&files);
        let read = job(&body, Origin::Listed { offset: 100 }, FileList::Records).unwrap();
        assert_eq!(read.problems, Vec::<String>::new());
        assert_eq!(read.end, body.len());
        let job = read.job;
        assert_eq!(
            job.id.as_deref(),
            Some("04030201-0605-0807-090a-0b0c0d0e0f10")
        );
        assert_eq!(job.kind, Some(JobType::Download));
        assert_eq!(job.priority, Some(Priority::Normal));
        assert_eq!(job.state, Some(State::Transferred));
        assert_eq!(job.name.as_deref(), Some("Setup"));
        assert_eq!(job.notify_program.as_deref(), Some("C:\\evil.exe"));
        assert_eq!(job.notify_arguments.as_deref(), Some("/c x"));
        assert_eq!(job.owner.as_deref(), Some("S-1-5-18"));
        assert_eq!(job.notify_flags, Some(11));
        assert_eq!((job.retry_delay, job.no_progress_timeout), (600, 1_209_600));
        assert_eq!(job.created, filetime(time(0)));
        assert_eq!(job.completed, filetime(time(5)));
        assert_eq!(job.expires, filetime(time(5 + 90 * 86_400)));
        assert_eq!(job.file_count, Some(2));
        assert_eq!(job.files.len(), 2);
        let second = &job.files[1];
        assert_eq!(second.remote_url.as_deref(), Some("http://a/2"));
        assert_eq!(second.bytes_total, Some(20));
        assert_eq!(second.volume_serial, Some(0xCA0C_7A48));
        assert_eq!(second.remote_modified, filetime(time(0)));
        let first_file_at = body.len() - metadata_record().len() - 16 - 2 * files[0].len();
        assert_eq!(
            job.files[0].origin,
            Origin::Listed {
                offset: 100 + first_file_at
            }
        );
    }

    #[test]
    fn reports_missing_files() {
        let mut body = control_record("x", "");
        body.extend(marker::FILES);
        body.extend(2u32.to_le_bytes());
        body.extend(file_record("http://a", 1));
        body.extend(marker::FILES);
        body.extend(metadata_record());
        let read = job(&body, Origin::Carved { offset: 0 }, FileList::Records).unwrap();
        assert_eq!(read.job.files.len(), 1);
        assert_eq!(read.problems, ["2 files listed, 1 read"]);
    }

    #[test]
    fn reads_file_ids() {
        let mut body = control_record("x", "");
        body.extend(marker::FILES);
        body.extend(1u32.to_le_bytes());
        body.extend([0xAB; 16]);
        body.extend(marker::FILES);
        body.extend(metadata_record());
        let read = job(&body, Origin::Row { page: 3 }, FileList::Ids).unwrap();
        assert_eq!(read.job.file_ids, ["abababab-abab-abab-abab-abababababab"]);
        assert!(read.job.files.is_empty());
    }

    #[test]
    fn unknown_sizes_and_values() {
        assert_eq!(JobType::from_u32(7), JobType::Other(7));
        assert_eq!(Priority::from_u32(9), Priority::Other(9));
        assert_eq!(State::from_u32(8), State::Cancelled);
        let mut record = file_record("u", 0);
        let total_at = stored("C:\\Users\\a\\x.exe").len()
            + stored("u").len()
            + stored("C:\\Users\\a\\BIT1.tmp").len()
            + 8;
        record[total_at..total_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let (file, _) = file(&record, 0, Origin::Row { page: 1 }).unwrap();
        assert_eq!(file.bytes_total, None);
    }

    #[test]
    fn damage_is_an_error_not_a_panic() {
        let body = job_record(&[file_record("http://a", 1)]);
        for cut in 0..body.len() {
            let _ = job(
                &body[..cut],
                Origin::Carved { offset: 0 },
                FileList::Records,
            );
        }
        assert!(job(&body[..10], Origin::Carved { offset: 0 }, FileList::Records).is_err());
    }
}
