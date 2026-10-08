"""BitsParser's JSON output (a stream of objects) as the TSV tests/oracle.rs reads.

One line per item, tab-separated, the record's number first:

    <n>  job    id name description type priority state owner program arguments created modified
    <n>  noise  (a record whose type isn't one BITS defines: parsed noise)
    <n>  times  created modified   (a record without a job id: only files and times)
    <n>  file   local-path remote-url temp-path transferred total volume

Empty where BitsParser gives nothing (it leaves out empty values, and sizes
of all ones).
"""

import json
import sys

JOB = ["JobId", "JobName", "JobDesc", "JobType", "JobPriority", "JobState",
       "OwnerSID", "CommandExecuted", "CommandArguments", "CreationTime", "ModifiedTime"]
FILE = ["DestFile", "SourceURL", "TmpFile", "DownloadByteSize", "TransferByteSize", "VolumeGUID"]


def records(text):
    decoder = json.JSONDecoder()
    at = 0
    while True:
        while at < len(text) and text[at].isspace():
            at += 1
        if at >= len(text):
            return
        record, at = decoder.raw_decode(text, at)
        yield record


def line(number, kind, record, keys):
    values = [str(record.get(key, "")) for key in keys]
    for value in values:
        if "\t" in value or "\n" in value:
            raise ValueError(f"record {number}: a tab or newline in {value!r}")
    return "\t".join([str(number), kind] + values)


def main():
    out = []
    for number, record in enumerate(records(open(sys.argv[1], encoding="utf-8").read()), 1):
        if "JobId" in record:
            kind = "job" if isinstance(record.get("JobType"), str) else "noise"
            out.append(line(number, kind, record, JOB))
        else:
            out.append(line(number, "times", record, ["CreationTime", "ModifiedTime"]))
        for file in record.get("Files", []):
            out.append(line(number, "file", file, FILE))
    sys.stdout.write("\n".join(out) + "\n")


main()
