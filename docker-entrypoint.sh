#!/bin/sh
set -eu

DB_FILE="${HERMES_DB:-sqlite://hermes.db}"
DB_FILE="${DB_FILE#sqlite://}"

# Scan before opening the port only when there is nothing to serve yet. Serving an empty
# table is worse than making the first visitor wait, and this is the state the very first
# deploy starts in — or every deploy, if the volume is ever missing. Once the database has
# rows the branch is skipped and the port opens immediately.
if [ ! -s "$DB_FILE" ]; then
  hermes scan || echo "hermes: initial scan failed; serving an empty store" >&2
  first_delay="${HERMES_SCAN_INTERVAL:-86400}"
else
  # There was already data, so refresh it now rather than serving whatever the last deploy
  # left behind until tomorrow.
  first_delay=0
fi

# Refresh in the background, never in front of the port. A scan that has to finish before
# serving starts leaves the health check unanswered for its whole duration, which is
# survivable at 62 seeded addresses and stops being survivable as the seed grows.
#
# The loop lives in this container rather than in a scheduled second service because Railway
# allows one volume per service, so a separate cron service could not reach this database.
(
  sleep "$first_delay"
  while true; do
    hermes scan || echo "hermes: scan failed; keeping the previous results" >&2
    sleep "${HERMES_SCAN_INTERVAL:-86400}"
  done
) &

exec hermes serve
