#!/bin/sh
set -eu

# Serve in the foreground and scan behind it, never the other way round.
#
# A scan that runs to completion before the port opens means the health check gets no answer
# until it finishes. That is survivable at 62 seeded addresses and stops being survivable as
# the seed grows, at which point the container is killed mid-scan and restarted forever.
#
# The refresh loop lives in this container rather than in a scheduled second service because
# Railway allows one volume per service, so a separate cron service could not reach this
# database at all.
(
  while true; do
    hermes scan || echo "hermes: scan failed; keeping the previous results" >&2
    sleep "${HERMES_SCAN_INTERVAL:-86400}"
  done
) &

exec hermes serve
