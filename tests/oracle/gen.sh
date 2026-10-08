#!/bin/sh
# Writes the expected values tests/oracle.rs compares with: FireEye's
# BitsParser (Apache-2.0), run on the fixtures with carving on (--carvedb).
#
# - Queue files: BitsParser at fireeye/BitsParser@0a2b51ee.
# - qmgr.db: the original fails on Windows 11's 16 KiB pages, so its fork
#   andreisss/BitParser@ed1a894b (Apache-2.0), which reads them, is run.
#
# Both use ANSSI's bits_parser 1.0.1 (MIT) from PyPI. Needs git, gzip and
# python3; everything is downloaded to a temporary directory and removed.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
fixtures="$here/../fixtures"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

checkout() { # repository commit directory
    git clone -q "https://github.com/$1.git" "$work/$3"
    git -C "$work/$3" checkout -q "$2"
}
checkout fireeye/BitsParser 0a2b51eeec79c5e181d8fc526d5715552299c45f original
checkout andreisss/BitParser ed1a894b6996f4877d1bdf38cd51a02d64cfc73b fork
python3 -m venv "$work/venv"
"$work/venv/bin/python" -I -m pip install -q bits_parser==1.0.1

run() { # checkout fixture name
    gzip -dc "$fixtures/$2.gz" > "$work/$3"
    "$work/venv/bin/python" -I "$here/run_bitsparser.py" "$work/$1" \
        -i "$work/$3" --carvedb -o "$work/$3.json" 2> /dev/null
    python3 -I "$here/bitsparser_tsv.py" "$work/$3.json" > "$here/$3.tsv"
}
run original cfreds/qmgr0.dat qmgr0.dat
run original cfreds/qmgr1.dat qmgr1.dat
run fork go-ese/qmgr.db qmgr.db
