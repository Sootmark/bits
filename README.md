# bits

Windows BITS (Background Intelligent Transfer Service) job queues, for forensics: the jobs Windows, updaters and attackers queue to download or upload files, read without Windows. Each job's owner, files (URL, local and temporary paths, sizes), times, and the program BITS runs when the job completes or fails (`SetNotifyCmdLine`, set by `bitsadmin /SetNotifyCmdLine`), a known way to persist and to run again as `SYSTEM`. Jobs deleted or rewritten are carved from the queue's free space. Two dependencies, its siblings `sootmark-common` (times, GUIDs) and `sootmark-ese` (the ESE database of Windows 10 and later).

```toml
[dependencies]
sootmark-bits = "0.1"
```

```rust
let file = std::fs::read(r"C:\ProgramData\Microsoft\Network\Downloader\qmgr.db")?;
let queue = bits::read(&file)?;
for job in queue.jobs.iter().chain(&queue.carved_jobs) {
    println!("{:?} {:?} owner {:?} runs {:?} {:?}", job.created, job.name, job.owner,
        job.notify_program, job.notify_arguments);
    for file in &job.files {
        println!("  {:?} -> {:?}", file.remote_url, file.local_path);
    }
}
```

## What you get

- `read(bytes)`: a queue in either format, `%ProgramData%\Microsoft\Network\Downloader\`:
  - `qmgr.db` (Windows 10 and later): the `Jobs` and `Files` tables of the ESE database, each job with the files it lists by GUID; `orphan_files`, the files no job lists;
  - `qmgr0.dat`, `qmgr1.dat` (Windows XP to 8.1): BITS's own queue file, every job of its lists, each followed by its files.
- Per job: id (GUID), name, description, type (download, upload, upload-reply), priority, state (`BG_JOB_STATE`), owner SID, the notify program and its arguments, notify flags, created, modified, completed and expiry times (UTC), retry delay and no-progress timeout (seconds), error and transient error counts, its file count, file GUIDs and files, and where it was read (table row, list offset, or carved offset).
- Per file: remote URL, local path, temporary `BIT*.tmp` path, bytes transferred and total (unknown while BITS doesn't know it), drive, volume GUID path, the volume's serial number, and the time BITS keeps of the remote file (to the second; its `Last-Modified`, as far as the samples show).
- `carved_jobs` and `carved_files`, recovered from what no live record holds (free pages of the database, the queue file past its lists), each once and left out when the same as a live one:
  - whole jobs after a job marker, kept when type, priority and state are values BITS defines and the times plausible;
  - jobs whose start was overwritten, found by the end of their files and the times after: the files whole when the list's start is still there, then the fields before read backwards as far as they are intact (owner, notify arguments and program, description, name, then type, priority, state and id); otherwise the files read backwards, the first one cut short marked `damaged` with what is left of its temporary path;
  - file records of `qmgr.db` after the file marker, with their GUID when the database record around them still gives it, and attached to the carved jobs that list them.
- `detect(name, head)`: a queue file by its content, `qmgr.db` by its name and ESE signature.
- Damage goes to `problems`, never a panic.

## How it's checked

- Velocidex go-ese's `qmgr.db` (Apache-2.0, a Windows 11 24H2 queue with 16 KiB pages, left dirty; `tests/fixtures/go-ese/`) and the `qmgr0.dat` and `qmgr1.dat` of NIST CFReDS's Data Leakage Case (a Windows 7 image, a US Government work; `tests/fixtures/cfreds/`), see each `NOTICE`.
- Against FireEye's [BitsParser](https://github.com/fireeye/BitsParser) (Apache-2.0) with carving on: every record it prints is read the same, field by field (`tests/oracle/`, written by `tests/oracle/gen.sh`): from `qmgr.db` its 48 jobs and 76 files (BitsParser itself crashes on 16 KiB pages; its fork [andreisss/BitParser](https://github.com/andreisss/BitParser) reads them), from `qmgr1.dat` the listed job and the carved one, from `qmgr0.dat` its two carved jobs, their 8 files between them. Beyond BitsParser: the files of the live `qmgr.db` job (its ESE reader returns garbage rows there), 4 jobs whose job marker an ESE page header overwrote, 4 more with only their times and file GUIDs left, and 6 records BitsParser prints with impossible types are left out.
- Fields BitsParser doesn't print, checked against the evidence: the volume serial number of every Windows 7 file is the image's NTFS volume serial (`fsstat`); every one of the 60 jobs expires exactly 90 days after its last modification, as BITS's default `JobInactivityTimeout` has it.
- Unit tests on records written as the samples show them, and property tests: arbitrary bytes, the queue files and the database cut anywhere or with bytes changed never panic.

## Not yet

- The Windows 10 and later fields after the expiry time (the creating program's path, its package name, the HTTP method), the job's security descriptor, its errors' codes, and the credentials and custom headers it keeps (DPAPI blobs).
- Replaying `qmgr.db`'s transaction logs (`edb*.log`): a database copied from a running system may lack its latest changes (reported as a dirty shutdown). Deleted rows are reached by carving, not by walking the ESE free space trees.
- BITS versions before Windows 7: their queue files follow the same layout as far as the documentation of other tools goes, but no open sample was found; the 36 bytes ending each Windows 7 file record are assumed.

## Licence

MIT or Apache-2.0, at your option. The test files are go-ese's (Apache-2.0) and NIST's (public domain in the United States), see the `NOTICE` files. BitsParser is only run to write the expected values, never included.
