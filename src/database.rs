//! `qmgr.db` (Windows 10 and later): an ESE database whose `Jobs` and
//! `Files` tables each have an `Id` (GUID) and a `Blob`. A job's blob is
//! its record between two copies of a marker of the BITS version; a file's
//! blob its record between two copies of the file marker. The job lists
//! its files' `Id`s.

use common::win::guid_to_string;
use ese::{Database, Row, Table, Value};

use crate::marker;
use crate::record::{self, File, FileList, Job, Origin};

/// The tables.
pub(crate) const JOBS: &str = "Jobs";
pub(crate) const FILES: &str = "Files";

/// The jobs of the `Jobs` table, their files from the `Files` table, and
/// the files no job lists.
pub(crate) struct Tables {
    pub(crate) jobs: Vec<Job>,
    pub(crate) orphan_files: Vec<File>,
    pub(crate) problems: Vec<String>,
}

/// Whether `db` has the BITS tables.
pub(crate) fn is_bits(db: &Database<'_>) -> bool {
    db.table(JOBS).is_some() && db.table(FILES).is_some()
}

/// Every job of `db`, with its files.
pub(crate) fn read(db: &Database<'_>) -> Tables {
    let mut problems = db.problems.clone();
    let files = rows(db, FILES, &mut problems, file_row);
    let mut jobs = rows(db, JOBS, &mut problems, job_row);
    let mut listed = vec![false; files.len()];
    for job in &mut jobs {
        for id in &job.file_ids {
            match files.iter().position(|file| file.id.as_ref() == Some(id)) {
                Some(at) => {
                    listed[at] = true;
                    job.files.push(files[at].clone());
                }
                None => problems.push(format!(
                    "job {}: its file {id} isn't in the {FILES} table",
                    job.id.as_deref().unwrap_or("?")
                )),
            }
        }
    }
    let orphan_files = files
        .into_iter()
        .zip(listed)
        .filter_map(|(file, listed)| (!listed).then_some(file))
        .collect();
    Tables {
        jobs,
        orphan_files,
        problems,
    }
}

/// What a row gave: its value and what was wrong in it.
type Converted<T> = Result<(T, Vec<String>), String>;

/// Each row of `table` that `convert` reads.
fn rows<T>(
    db: &Database<'_>,
    table: &str,
    problems: &mut Vec<String>,
    convert: fn(&Table, &Row) -> Converted<T>,
) -> Vec<T> {
    let (Some(definition), Ok(mut rows)) = (db.table(table), db.rows(table)) else {
        return Vec::new();
    };
    let mut read = Vec::new();
    for row in rows.by_ref() {
        let context = format!("{table} row on page {}", row.page);
        match convert(definition, &row) {
            Ok((value, damage)) => {
                read.push(value);
                problems.extend(damage.into_iter().map(|p| format!("{context}: {p}")));
            }
            Err(problem) => problems.push(format!("{context}: {problem}")),
        }
    }
    problems.extend(rows.problems().iter().map(|p| format!("{table}: {p}")));
    read
}

/// A `Files` row's file.
fn file_row(table: &Table, row: &Row) -> Converted<File> {
    let blob = blob(table, row, &[marker::DATABASE_FILE])?;
    let (mut file, _) = record::file(blob, marker::SIZE, Origin::Row { page: row.page })
        .ok_or("its record doesn't read")?;
    file.id = id(table, row);
    Ok((file, Vec::new()))
}

/// A `Jobs` row's job.
fn job_row(table: &Table, row: &Row) -> Converted<Job> {
    let blob = blob(table, row, &marker::DATABASE_JOBS)?;
    let mut read = record::job(
        &blob[marker::SIZE..],
        Origin::Row { page: row.page },
        FileList::Ids,
    )?;
    let row_id = id(table, row);
    if row_id.is_some() && row_id != read.job.id {
        read.problems.push(format!(
            "its Id {} isn't its job's {}",
            row_id.unwrap_or_default(),
            read.job.id.as_deref().unwrap_or_default()
        ));
    }
    Ok((read.job, read.problems))
}

/// The row's `Blob`, checked to start with one of `markers`.
fn blob<'r>(
    table: &Table,
    row: &'r Row,
    markers: &[[u8; marker::SIZE]],
) -> Result<&'r [u8], String> {
    let blob = row
        .get(table, "Blob")
        .and_then(Value::as_bytes)
        .ok_or("no Blob")?;
    if markers.iter().any(|m| marker::is_at(blob, 0, m)) {
        Ok(blob)
    } else {
        Err("its Blob doesn't start with a marker BITS writes".to_owned())
    }
}

/// The row's `Id`.
fn id(table: &Table, row: &Row) -> Option<String> {
    match row.get(table, "Id")? {
        Value::Guid(bytes) => Some(guid_to_string(bytes)),
        Value::Binary(bytes) => Some(guid_to_string(bytes.as_slice().try_into().ok()?)),
        _ => None,
    }
}
