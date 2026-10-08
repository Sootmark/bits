//! What the open samples hold, field by field, beyond what BitsParser
//! prints: the fields it leaves out, and the checks the image allows.

mod support;

use bits::{Format, JobType, Origin, Priority, State};
use support::{fixture, seconds};

/// The serial number of the Windows 7 image's NTFS volume (`fsstat`:
/// `C8CA0C8DCA0C7A48`), whose low 32 bits Windows shows as `CA0C-7A48`.
const IMAGE_VOLUME_SERIAL: u32 = 0xCA0C_7A48;
/// BITS deletes a job 90 days after its last change, by default.
const NINETY_DAYS_IN_TICKS: i64 = 90 * 86_400 * 10_000_000;

#[test]
fn the_windows_11_job_and_its_file() {
    let queue = bits::read(&fixture("go-ese/qmgr.db")).unwrap();
    assert_eq!(queue.format, Format::Database);
    assert_eq!(
        queue.problems,
        ["dirty shutdown: changes still only in the transaction log files are missing (logs are not replayed)"]
    );
    let job = &queue.jobs[0];
    assert_eq!(job.origin, Origin::Row { page: 31 });
    assert_eq!(
        job.id.as_deref(),
        Some("d387e452-e2b3-472d-b3b3-85c2be33cbb3")
    );
    assert_eq!(job.name.as_deref(), Some("Edge Component Updater"));
    assert_eq!(
        (job.kind, job.priority, job.state),
        (
            Some(JobType::Download),
            Some(Priority::Normal),
            Some(State::Transferred)
        )
    );
    assert_eq!(job.notify_program.as_deref(), Some(""));
    assert_eq!(job.notify_flags, Some(3));
    assert_eq!((job.retry_delay, job.no_progress_timeout), (60, 86_400));
    assert_eq!(seconds(job.completed), "2024-07-30T07:19:08Z");
    assert_eq!(seconds(job.expires), "2024-10-28T07:19:08Z");
    assert_eq!(job.file_ids, ["3f4a8b1a-98e5-4aa8-a2e7-c0bf1bc6f48d"]);
    let file = &job.files[0];
    assert_eq!(
        file.id.as_deref(),
        Some("3f4a8b1a-98e5-4aa8-a2e7-c0bf1bc6f48d")
    );
    assert!(file.remote_url.as_deref().unwrap().starts_with(
        "http://msedge.b.tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/2132f61f-"
    ));
    assert_eq!(
        file.temp_path.as_deref(),
        Some(r"C:\Users\bob\AppData\Local\Temp\edge_BITS_2972_515758394\BIT2B52.tmp")
    );
    assert_eq!(
        (file.bytes_transferred, file.bytes_total),
        (975_576, Some(975_576))
    );
    assert_eq!(file.drive, r"C:\");
    assert_eq!(seconds(file.remote_modified), "2022-03-08T02:44:10Z");
    assert!(queue.orphan_files.is_empty());
}

#[test]
fn the_windows_7_job_and_its_older_copies() {
    let listed = bits::read(&fixture("cfreds/qmgr1.dat")).unwrap();
    assert_eq!(listed.format, Format::QueueFile);
    let job = &listed.jobs[0];
    assert_eq!(job.origin, Origin::Listed { offset: 68 });
    assert_eq!(job.name.as_deref(), Some("Setup Installer"));
    assert_eq!(job.notify_flags, Some(11));
    assert_eq!((job.retry_delay, job.no_progress_timeout), (600, 120));
    let file = &job.files[0];
    assert_eq!(file.volume_serial, Some(IMAGE_VOLUME_SERIAL));
    assert_eq!(seconds(file.remote_modified), "2010-03-19T00:29:02Z");

    // qmgr0.dat lists nothing; its free space keeps the same job, before
    // its transfer ended, its start overwritten.
    let empty = bits::read(&fixture("cfreds/qmgr0.dat")).unwrap();
    assert!(empty.jobs.is_empty());
    let older = &empty.carved_jobs[0];
    assert_eq!(older.origin, Origin::Carved { offset: 136 });
    assert_eq!((older.id.as_ref(), older.name.as_ref()), (None, None));
    assert_eq!(older.owner, job.owner);
    assert_eq!(older.description.as_deref(), Some(""));
    assert_eq!(older.created, job.created);
    assert_eq!(older.files[0].bytes_transferred, 0);
    assert_eq!(older.files[0].bytes_total, None);

    // An Outlook address book download whose first file lost its start.
    let outlook = &empty.carved_jobs[1];
    assert_eq!(outlook.file_count, None);
    assert!(outlook.files[0].damaged);
    assert_eq!(
        outlook.files[0].temp_path.as_deref(),
        Some(r"cf15-73c9-4426-8bf9-3f5b9b52ce92\BIT9A34.tmp")
    );
    assert_eq!(outlook.files.len(), 3);
    for file in &outlook.files {
        assert_eq!(file.volume_serial, Some(IMAGE_VOLUME_SERIAL));
    }
}

#[test]
fn every_job_expires_ninety_days_after_its_last_change() {
    let mut jobs = 0;
    for name in ["go-ese/qmgr.db", "cfreds/qmgr0.dat", "cfreds/qmgr1.dat"] {
        let queue = bits::read(&fixture(name)).unwrap();
        for job in queue.jobs.iter().chain(&queue.carved_jobs) {
            let modified = job.modified.and_then(|t| t.ticks()).unwrap();
            let expires = job.expires.and_then(|t| t.ticks()).unwrap();
            assert_eq!(expires - modified, NINETY_DAYS_IN_TICKS, "{name}: {job:?}");
            jobs += 1;
        }
    }
    assert_eq!(jobs, 60);
}
