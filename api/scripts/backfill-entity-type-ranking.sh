#!/usr/bin/env bash
#
# Populate (and later reconcile) entity_type_ranking for migration 0084.
#
# Deliberately NOT part of the migration: api's initContainer runs db:migrate, so a
# multi-million-row populate inside a migration stalls every deploy. 0073 established
# this and 0084's header repeats it.
#
# This does NOT recompute scores. It reads entity_ranking_scores as the source of truth
# and projects it across each entity's Types relations, which is why it takes seconds
# rather than the hours backfill-entity-ranking-scores.sh takes. If an entity has no
# score row it is correctly absent: the ranked feed JOINs entity_ranking_scores, so such
# an entity is already invisible to the feed.
#
# Batched by leading uuid byte of entity_id (256 batches) so each batch is a range scan
# in its own transaction. Idempotent and resumable: the DELETE removes pairs whose Types
# relation is gone and the INSERT upserts, so re-running a batch converges rather than
# duplicating. Resume with START=<n>.
#
# It is also the RECONCILER. Nothing calls refresh_entity_ranking_scores when a Types
# relation is added or removed (0084's header: a pre-existing gap, since ranking_score
# already contains log(type_weight)), so running this periodically repairs type drift.
#
# Usage:
#   DATABASE_URL=postgres://...  ./backfill-entity-type-ranking.sh
#   START=128 DATABASE_URL=...   ./backfill-entity-type-ranking.sh   # resume
#
# In-cluster (keeps the credential out of argv and shell history):
#   kubectl -n gaia exec -i <pod> -- sh -c 'DATABASE_URL="$PGURL" bash -s' \
#     < api/scripts/backfill-entity-type-ranking.sh
#
# Progress is measured against the DB, not inferred from the loop, because a batch that
# reports success is not proof that rows changed.
set -uo pipefail

: "${DATABASE_URL:?DATABASE_URL is required}"
START=${START:-0}
TYPES_PROP='8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
# ON_ERROR_STOP is load-bearing: psql exits 0 even when a statement fails, so without it
# a broken batch is indistinguishable from one that wrote 0 rows.
PSQL=(psql -v ON_ERROR_STOP=1 -t -A "$DATABASE_URL")

echo "$(date -u +%H:%M:%S) start rows=$("${PSQL[@]}" -c 'SELECT count(*) FROM public.entity_type_ranking')"

ins=0
del=0
fail=0
for i in $(seq "$START" 255); do
  lo=$(printf '%02x000000-0000-0000-0000-000000000000' "$i")
  if [ "$i" -eq 255 ]; then
    hi="ffffffff-ffff-ffff-ffff-ffffffffffff"; op="<="
  else
    hi=$(printf '%02x000000-0000-0000-0000-000000000000' $((i + 1))); op="<"
  fi

  # psql runs a multi-statement -c as ONE implicit transaction, so no explicit BEGIN is
  # needed and adding one only produces a "transaction already in progress" warning. That
  # atomicity is the point: a mid-batch failure cannot leave the range half reconciled.
  #
  # DELETE before INSERT, by absence rather than by clearing the range: clearing would
  # make every entity in the range briefly vanish from its type for any concurrent reader.
  out=$("${PSQL[@]}" -c "
    SET statement_timeout = 0;
    WITH gone AS (
      DELETE FROM public.entity_type_ranking etr
      WHERE etr.entity_id >= '${lo}'::uuid AND etr.entity_id ${op} '${hi}'::uuid
        AND NOT EXISTS (
          SELECT 1 FROM public.relations r
          WHERE r.from_entity_id = etr.entity_id
            AND r.type_id = '${TYPES_PROP}'::uuid
            AND r.to_entity_id = etr.type_id
        )
      RETURNING 1
    ) SELECT count(*) FROM gone;
    WITH put AS (
      INSERT INTO public.entity_type_ranking (type_id, entity_id, ranking_score)
      SELECT DISTINCT r.to_entity_id, r.from_entity_id, ers.ranking_score
      FROM public.relations r
      JOIN public.entity_ranking_scores ers ON ers.entity_id = r.from_entity_id
      WHERE r.from_entity_id >= '${lo}'::uuid AND r.from_entity_id ${op} '${hi}'::uuid
        AND r.type_id = '${TYPES_PROP}'::uuid
      ON CONFLICT (type_id, entity_id) DO UPDATE SET
        ranking_score = EXCLUDED.ranking_score
      RETURNING 1
    ) SELECT count(*) FROM put;" 2>&1)
  rc=$?

  # Two counts, in statement order: deleted then upserted.
  counts=$(printf '%s' "$out" | grep -oE '^[0-9]+$')
  d=$(printf '%s' "$counts" | sed -n 1p)
  n=$(printf '%s' "$counts" | sed -n 2p)
  if [ $rc -ne 0 ] || [ -z "$n" ] || [ -z "$d" ]; then
    echo "BATCH $i ($lo) FAILED rc=$rc: $out" >&2
    fail=$((fail + 1))
  else
    ins=$((ins + n))
    del=$((del + d))
  fi

  if [ $((i % 16)) -eq 0 ] || [ "$i" -eq 255 ]; then
    echo "$(date -u +%H:%M:%S) batch $i/255  upserted=$ins  deleted=$del  failures=$fail"
  fi
done

# The planner needs this before the new index is trusted; without it the first typed
# query can still choose a seq scan.
"${PSQL[@]}" -c 'ANALYZE public.entity_type_ranking;' >/dev/null 2>&1

echo "DONE upserted=$ins deleted=$del failures=$fail"
echo "rows=$("${PSQL[@]}" -c 'SELECT count(*) FROM public.entity_type_ranking')"
echo "types=$("${PSQL[@]}" -c 'SELECT count(DISTINCT type_id) FROM public.entity_type_ranking')"

if [ "$fail" -gt 0 ]; then
  echo "Re-run the failed ranges with START=<first failed batch>; the reconcile makes it safe." >&2
  exit 1
fi
