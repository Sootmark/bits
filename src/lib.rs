//! Windows BITS (Background Intelligent Transfer Service) job queues, for
//! forensics: the jobs Windows, updaters and attackers queue to download or
//! upload files, with their owner, their files (URL, local and temporary
//! paths, sizes), their times, and the program BITS runs when a job
//! completes or fails (`SetNotifyCmdLine`), a known way to persist.
//!
//! - `qmgr.db` (Windows 10 and later,
//!   `%ProgramData%\Microsoft\Network\Downloader`): an ESE database, read
//!   with `sootmark-ese`.
//! - `qmgr0.dat` and `qmgr1.dat` (Windows XP to 8.1, same folder): BITS's
//!   own queue file.
//!
//! [`read`] gives the jobs the queue lists, and those carved from its free
//! space (deleted jobs, older versions of jobs). Damage goes to
//! [`Queue::problems`], never a panic.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let file = std::fs::read(r"C:\ProgramData\Microsoft\Network\Downloader\qmgr.db")?;
//! let queue = bits::read(&file)?;
//! for job in queue.jobs.iter().chain(&queue.carved_jobs) {
//!     println!("{:?} {:?} run when done: {:?}", job.name, job.owner, job.notify_program);
//!     for file in &job.files {
//!         println!("  {:?} -> {:?}", file.remote_url, file.local_path);
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod carve;
mod database;
mod marker;
mod queue;
mod record;
mod text;

pub use record::{File, Job, JobType, Origin, Priority, State};

use carve::Layout;
use record::FileList;

/// This crate's version, for records of what parsed them.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Offset and bytes of the ESE signature.
const ESE_SIGNATURE_AT: usize = 4;
const ESE_SIGNATURE: [u8; 4] = [0xEF, 0xCD, 0xAB, 0x89];

/// Which format a queue is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// `qmgr.db`, an ESE database (Windows 10 and later).
    Database,
    /// `qmgr0.dat` or `qmgr1.dat` (Windows XP to 8.1).
    QueueFile,
}

/// Why a file can't be read as a BITS queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Neither an ESE database nor a queue file.
    NotAQueue,
    /// An ESE database without the `Jobs` and `Files` tables.
    NotBits,
    /// An ESE database that can't be opened.
    Database(ese::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAQueue => f.write_str("not a BITS queue (neither qmgr.db nor qmgr0.dat)"),
            Self::NotBits => f.write_str("an ESE database without BITS's Jobs and Files tables"),
            Self::Database(e) => write!(f, "the database can't be opened: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// What a queue holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    /// Its format.
    pub format: Format,
    /// The jobs it lists (`qmgr.db`: the rows of the `Jobs` table), with
    /// their files.
    pub jobs: Vec<Job>,
    /// Files of `qmgr.db`'s `Files` table that no job lists.
    pub orphan_files: Vec<File>,
    /// Jobs recovered from free space that differ from every listed job:
    /// deleted jobs and older versions of jobs, each once. Their files are
    /// attached as for listed jobs, from the listed or carved files.
    pub carved_jobs: Vec<Job>,
    /// File records recovered from free space (`qmgr.db`) that differ from
    /// every listed file and that no job claims, each once.
    pub carved_files: Vec<File>,
    /// What couldn't be read, and why.
    pub problems: Vec<String>,
}

/// Which format a file named `name` (a path or a bare name) starting with
/// `head` (its first 32 bytes are enough) is in: a queue file by its
/// content, `qmgr.db` by its name and ESE signature.
#[must_use]
pub fn detect(name: &str, head: &[u8]) -> Option<Format> {
    if queue::is_queue(head) {
        return Some(Format::QueueFile);
    }
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let ese = head.get(ESE_SIGNATURE_AT..ESE_SIGNATURE_AT + ESE_SIGNATURE.len())
        == Some(&ESE_SIGNATURE[..]);
    (ese && base.eq_ignore_ascii_case("qmgr.db")).then_some(Format::Database)
}

/// The jobs of a queue, `qmgr.db` or `qmgr0.dat`/`qmgr1.dat`, and those
/// carved from its free space.
///
/// # Errors
/// [`Error::NotAQueue`] for a file in neither format,
/// [`Error::Database`] for an ESE database that can't be opened, and
/// [`Error::NotBits`] for one without BITS's tables. Damage beyond that is
/// listed in [`Queue::problems`].
pub fn read(file: &[u8]) -> Result<Queue, Error> {
    if queue::is_queue(file) {
        Ok(read_queue_file(file))
    } else if file.get(ESE_SIGNATURE_AT..ESE_SIGNATURE_AT + ESE_SIGNATURE.len())
        == Some(&ESE_SIGNATURE[..])
    {
        read_database(file)
    } else {
        Err(Error::NotAQueue)
    }
}

fn read_queue_file(file: &[u8]) -> Queue {
    let lists = queue::read(file);
    let layout = Layout {
        job_markers: &marker::LEGACY_JOBS,
        list: FileList::Records,
    };
    let carved = carve::carve(file, &layout, 0..lists.end);
    let mut queue = Queue {
        format: Format::QueueFile,
        jobs: lists.jobs,
        orphan_files: Vec::new(),
        carved_jobs: Vec::new(),
        carved_files: Vec::new(),
        problems: lists.problems,
    };
    keep_new(&mut queue, carved);
    queue
}

fn read_database(file: &[u8]) -> Result<Queue, Error> {
    let db = ese::Database::open(file).map_err(Error::Database)?;
    if !database::is_bits(&db) {
        return Err(Error::NotBits);
    }
    let tables = database::read(&db);
    let layout = Layout {
        job_markers: &marker::DATABASE_JOBS,
        list: FileList::Ids,
    };
    let carved = carve::carve(file, &layout, 0..0);
    let mut queue = Queue {
        format: Format::Database,
        jobs: tables.jobs,
        orphan_files: tables.orphan_files,
        carved_jobs: Vec::new(),
        carved_files: Vec::new(),
        problems: tables.problems,
    };
    keep_new(&mut queue, carved);
    Ok(queue)
}

/// Adds to `queue` what carving found that it doesn't already hold: files
/// attached to the carved jobs that list them, then each new job and file
/// once.
fn keep_new(queue: &mut Queue, carved: carve::Carved) {
    let carve::Carved { mut jobs, files } = carved;
    let listed_files: Vec<&File> = queue
        .jobs
        .iter()
        .flat_map(|job| &job.files)
        .chain(&queue.orphan_files)
        .collect();
    let mut claimed = vec![false; files.len()];
    for job in &mut jobs {
        for id in &job.file_ids {
            let has = |file: &&File| file.id.as_ref() == Some(id);
            if let Some(file) = listed_files.iter().copied().find(has) {
                job.files.push(file.clone());
            } else if let Some(at) = files.iter().position(|file| has(&file)) {
                claimed[at] = true;
                job.files.push(files[at].clone());
            }
        }
    }
    for job in jobs {
        let known = queue
            .jobs
            .iter()
            .chain(&queue.carved_jobs)
            .any(|k| k.same_as(&job));
        if !known {
            queue.carved_jobs.push(job);
        }
    }
    for (file, claimed) in files.into_iter().zip(claimed) {
        let known = listed_files.iter().any(|k| k.same_as(&file))
            || queue.carved_files.iter().any(|k| k.same_as(&file));
        if !claimed && !known {
            queue.carved_files.push(file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::queue_file;
    use crate::record::tests::{file_record, job_record};

    #[test]
    fn detection() {
        let queue = queue_file(&[], &[]);
        assert_eq!(detect("qmgr1.dat", &queue), Some(Format::QueueFile));
        let mut ese = vec![0; 32];
        ese[4..8].copy_from_slice(&ESE_SIGNATURE);
        assert_eq!(
            detect(r"C:\ProgramData\Microsoft\Network\Downloader\qmgr.db", &ese),
            Some(Format::Database)
        );
        assert_eq!(detect("SRUDB.dat", &ese), None);
        assert_eq!(detect("qmgr.db", b"short"), None);
    }

    #[test]
    fn queue_files_and_their_free_space() {
        let live = job_record(&[file_record("http://live", 1)]);
        let mut free = marker::LEGACY_JOBS[0].to_vec();
        free.extend(&live);
        free.extend(marker::LEGACY_JOBS[0]);
        free.extend(marker::LEGACY_JOBS[0]);
        free.extend(job_record(&[file_record("http://old", 2)]));
        free.extend(marker::LEGACY_JOBS[0]);
        let queue = read(&queue_file(&[live], &free)).unwrap();
        assert_eq!(queue.format, Format::QueueFile);
        assert_eq!(queue.jobs.len(), 1);
        assert_eq!(
            queue.carved_jobs.len(),
            1,
            "the copy of the live job is left out"
        );
        assert_eq!(
            queue.carved_jobs[0].files[0].remote_url.as_deref(),
            Some("http://old")
        );
        assert!(queue.problems.is_empty());
    }

    #[test]
    fn errors() {
        assert_eq!(read(b"not a queue"), Err(Error::NotAQueue));
        let mut ese = vec![0; 32];
        ese[4..8].copy_from_slice(&ESE_SIGNATURE);
        assert!(matches!(read(&ese), Err(Error::Database(_))));
        assert!(Error::NotBits.to_string().contains("Jobs"));
    }
}
