#!/usr/bin/env bash
# Download the full Wikidata JSON dump into data/. Safe to stop (Ctrl-C or kill)
# and re-run: wget -c resumes from the bytes already on disk.
#
# The URL is pinned to one dated dump rather than latest-all.json.bz2, because
# "latest" is repointed weekly and resuming across that would corrupt the file.
set -euo pipefail
DUMP_DIR="${DUMP_DIR:-20261102}"
DUMP_FILE="${DUMP_FILE:-wikidata-20261105-all.json.bz2}"
cd "$(dirname "$0")/../data"
exec wget -c --tries=0 --retry-connrefused --waitretry=30 --timeout=60 \
  --progress=dot:giga -a wget.log \
  "https://dumps.wikimedia.org/wikidatawiki/entities/${DUMP_DIR}/${DUMP_FILE}"
