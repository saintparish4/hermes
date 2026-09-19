#!/bin/sh
set -eu

DB_FILE="${HERMES_DB:-sqlite://hermes.db}"
DB_FILE="${DB_FILE#sqlite://}"

# Scan before opening the port only when there is nothing to serve yet. Serving an empty
# table is worse than making the first visitor wait, and this is the state the very first
# deploy starts in — or every deploy, if the volume is ever missing. Once the database has
# rows the branch is skipped and the port opens immediately.
#
# That first pass only classifies the hand-curated seed. Resolution reads Ethereum as well as
# Base and is paced to what the public endpoint tolerates, so a full first scan takes minutes and
# would outlast the 60-second health check, failing the deploy. Classifying the curated seed
# takes about twenty seconds; the port opens on real rows, and the full discovery and resolution
# pass starts straight away in the background.
#
# Twenty seconds is what the public endpoint gives when it is well. Rate-limited, the same pass
# took over two minutes of honest retries, so it is capped at 40 seconds and written in batches
# of ten: whatever it finished is served, and a slow endpoint costs rows rather than the deploy.
if [ ! -s "$DB_FILE" ]; then
  timeout 40 hermes scan --classify-only --batch 10 \
    || echo "hermes: initial classification failed or ran out of time; serving what it stored" >&2
else
  # Migrate once, alone, before the refresh and the server open the file together. Store::open
  # is safe to race, but a deploy that adds a column is exactly when two processes would
  # otherwise both be changing the schema. This has to stay inside this branch: run before the
  # emptiness check, it would create the schema, the file would no longer be empty, and a fresh
  # volume would open the port on an empty table.
  hermes migrate
fi

# Refresh in the background, never in front of the port, and start now rather than serving
# whatever the last deploy left behind until tomorrow. A scan killed by a redeploy resumes on
# the next boot: it skips everything scanned in the last twenty hours.
#
# Discovery runs first and is allowed to fail: an unreadable window stops it without losing its
# place, and the scan still covers everything already seeded.
#
# The loop lives in this container rather than in a scheduled second service because Railway
# allows one volume per service, so a separate cron service could not reach this database.
(
  while true; do
    hermes discover || echo "hermes: discovery failed; scanning what is already seeded" >&2
    hermes scan || echo "hermes: scan failed; keeping the previous results" >&2
    sleep "${HERMES_SCAN_INTERVAL:-86400}"
  done
) &

exec hermes serve
