#!/usr/bin/env bash
#
# Backfill entity_ranking_scores for the Explore feed "Best" sort (Phase A).
#
# Deliberately NOT part of migration 0074: api's initContainer runs db:migrate, so
# a 48.9M-row backfill inside a migration would stall every deploy.
#
# Batched by leading uuid byte (256 batches, ~190k entities each) so every batch is
# a primary-key range scan in its own transaction. Idempotent and resumable:
# refresh_entity_ranking_scores() upserts, so re-running a batch recomputes the same
# values rather than duplicating or double-counting. Resume with START=<n>.
#
# Usage:
#   DATABASE_URL=postgres://...  ./backfill-entity-ranking-scores.sh
#   START=128 DATABASE_URL=...   ./backfill-entity-ranking-scores.sh   # resume
#
# In-cluster (keeps the credential out of argv and shell history):
#   kubectl -n gaia exec -i <pod> -- sh -c 'DATABASE_URL="$PGURL" bash -s' \
#     < api/scripts/backfill-entity-ranking-scores.sh
#
# Progress is measured against the DB, not inferred from the loop, because a batch
# that reports success is not proof that rows changed.
set -uo pipefail

: "${DATABASE_URL:?DATABASE_URL is required}"
START=${START:-0}
# ON_ERROR_STOP is load-bearing: psql exits 0 even when a statement fails, so
# without it a broken batch is indistinguishable from one that scored 0 rows.
PSQL=(psql -v ON_ERROR_STOP=1 -t -A "$DATABASE_URL")

total=0
fail=0
for i in $(seq "$START" 255); do
  lo=$(printf '%02x000000-0000-0000-0000-000000000000' "$i")
  if [ "$i" -eq 255 ]; then
    hi="ffffffff-ffff-ffff-ffff-ffffffffffff"; op="<="
  else
    hi=$(printf '%02x000000-0000-0000-0000-000000000000' $((i + 1))); op="<"
  fi

  out=$("${PSQL[@]}" -c "
    SET statement_timeout = 0;
    SELECT public.refresh_entity_ranking_scores(
      ARRAY(SELECT id FROM public.entities
             WHERE id >= '${lo}'::uuid AND id ${op} '${hi}'::uuid)
    );" 2>&1)
  rc=$?

  n=$(printf '%s' "$out" | grep -oE '^[0-9]+$' | tail -1)
  if [ $rc -ne 0 ] || [ -z "$n" ]; then
    echo "BATCH $i ($lo) FAILED rc=$rc: $out" >&2
    fail=$((fail + 1))
  else
    total=$((total + n))
  fi

  if [ $((i % 16)) -eq 0 ] || [ "$i" -eq 255 ]; then
    echo "$(date -u +%H:%M:%S) batch $i/255  scored=$total  failures=$fail"
  fi
done

echo "DONE scored=$total failures=$fail"
if [ "$fail" -gt 0 ]; then
  echo "Re-run the failed ranges with START=<first failed batch>; the upsert makes it safe." >&2
  exit 1
fi
