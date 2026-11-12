# Best debates ranking: shadow week, cutover and rollback (GEO-3681)

Migration 0124 lets the top of Best follow the Debates space's Best debates ranking instead of agendas. It ships
**off**: `entity_ranking_config.ranking_places_mode = 0`, `trending.ranking_place_sources` empty and
`ranking_pushdown_since = 0`. Merging and deploying it changes no score. Everything below is an operator step
against the live database, in order. Each step says what to check before going on.

The one switch is `ranking_places_mode`:

| mode | what the hourly sweep does | what Best serves |
| -- | -- | -- |
| 0 off (shipped) | nothing for the ranking; empties its tables | 0122: agendas set debate places |
| 1 shadow | keeps `trending.ranking_places` / `ranking_pushdowns`, writes the daily report | unchanged: agendas set debate places |
| 2 on | as shadow, and re-scores debates whose place or push-down changed | ranking places + push-down; agendas stop setting places (their plain listing boost stays) |

A mode or weight change takes effect at the next **full pass**, which runs once a day. Every step that changes one
forces it.

**Who runs what.** SQL steps need write access to the gaia Postgres (`preston_ro` is read-only, so Patrick runs
them, or Preston with a write role). Forcing the full pass needs `kubectl` on the gaia cluster: Patrick, or Preston
with cluster access. Reading the report and checking the live API needs only read access: anyone. Each step below
says who.

## Before you start

- GEO-3685 has created the Best debates block in the Debates space (`9284477c8fe746368698994e2e517b32`) and added the
  rankers as members or editors. You need its block id: `SELECT id, name FROM ranks.ranking_blocks WHERE space_id =
  '9284477c-8fe7-4636-8698-994e2e517b32';`
- GEO-3676 (per-block row cap, average mode) is deployed, so `ranks.ranking_scores` holds the whole ranking for that
  block, keyed by entity.
- gaia#1496 (GEO-3676, migration 0123) is merged; this migration is 0124 and must apply after it.
- gaia#1483 (GEO-3631, a +10,000 tier for Debates-space agendas) stays closed and does not merge: it re-creates
  `entity_trending_terms` and `refresh_agenda_listings` from 0122's bodies, which would undo all of this. The 0124 SQL
  suite fails if either function stops reading `ranking_places_mode`.
- geogenesis already has the `BEST_DEBATES_RANKING_LIVE = false` constant
  (`apps/web/core/blocks/ranking/best-debates.ts`), landed with the Best debates web PRs (phase 1), so the cutover
  release only flips it.
- `psql "$DATABASE_URL"` is the gaia Postgres the `ranking-indexer-agenda-listings` CronJob uses.

### Forcing the full pass (used by every step; Patrick, or Preston with kubectl)

The Postgres pass and its geo-db twin are the same Job: the CronJob calls `trending.refresh_agenda_listings()` and then
mirrors the re-scored rows into geo-db (collection 21, with `GEODB_DERIVED_MIRROR`) and the full-pass marker
(collection 7, with `GEODB_STATE_MIRROR`). Make the next run a full pass, then run it now:

```sql
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
```

```bash
kubectl -n gaia create job --from=cronjob/ranking-indexer-agenda-listings agenda-listings-full-$(date +%Y%m%d%H%M)
kubectl -n gaia logs -f job/agenda-listings-full-<suffix>     # "agenda listings refreshed" with scored=N
```

(`SELECT trending.refresh_agenda_listings(true);` does the Postgres half by hand, but then geo-db waits for the next
hourly run. Prefer the Job.)

### Checking Best's top 10 against the live API (used by every step; anyone)

Save the top 10 before the change and compare within 1 hour after it:

```bash
API=https://api-testnet.geobrowser.io/graphql   # the live API for the environment you changed
top10() {
  curl -s "$API" -H 'content-type: application/json' --data '{"query":"{ entitiesRankedForFeedByTypeConnection(first: 10, typeIds: [\"fd51f935-2063-4617-be39-7b672b23364c\"]) { nodes { id rankingScore trendingScore trendingSource } } }"}' \
    | jq -r '.data.entitiesRankedForFeedByTypeConnection.nodes[] | "\(.id) \(.trendingScore) \(.trendingSource)"'
}
top10 > before.txt
# ... make the change and force the full pass ...
top10 > after.txt; diff before.txt after.txt
```

After the cutover the top 10 should be the ranking's order (compare with `trending.ranking_shadow_report`, below). After
a rollback it should be the agendas' order again.

## 1. Configure the sources (still off; Patrick)

Replace `<BLOCK_ID>` with the block's id. The values are first guesses to tune from the shadow report.

```sql
INSERT INTO trending.ranking_place_sources (name, space_id, block_id, weight, window_days, slots, day_factor, fade_from)
VALUES
  -- New: debates from the last 7 New York days, renumbered 1..20 among them, fading 0.65 a day.
  ('new',      '9284477c-8fe7-4636-8698-994e2e517b32', '<BLOCK_ID>', 1,   7,    20, 0.65, 'debate_created'),
  -- All time: the top 30, half weight, fading 0.97 a day (about half after 23 days).
  ('all_time', '9284477c-8fe7-4636-8698-994e2e517b32', '<BLOCK_ID>', 0.5, NULL, 30, 0.97, 'debate_created');

-- Phase 1 launch time (GEO-3591): only Debates members' downvotes cast at or after it count toward a push-down.
UPDATE entity_ranking_config SET ranking_pushdown_since = extract(epoch FROM timestamptz '<PHASE 1 LAUNCH, e.g. 2026-11-20 14:00-05>')::bigint WHERE id;
```

Check: nothing moves (the mode is still 0). `SELECT * FROM trending.current_ranking_places();` and
`SELECT * FROM trending.current_ranking_pushdowns();` show what shadow will store.

## 2. Shadow week (Patrick flips it; Preston reads the report)

```sql
UPDATE entity_ranking_config SET ranking_places_mode = 1 WHERE id;
```

Force the full pass. Check: Best's top 10 is unchanged; `SELECT count(*) FROM trending.ranking_places;` is above 0.

Every day's first run (the full pass) then writes the report:

```sql
-- The day: overlap of today's top 50 and the if-on top 50, places, push-downs, violations, and the cutover bar.
SELECT * FROM trending.ranking_shadow_days ORDER BY report_on DESC LIMIT 7;
-- Best today next to Best if on.
SELECT entity_id, served_rank, on_rank, ranking_rank, place_value, pushed_down
FROM trending.ranking_shadow_report WHERE report_on = trending.today()
ORDER BY on_rank;
-- The biggest moves from today to if-on.
SELECT * FROM trending.ranking_shadow_moves WHERE report_on = trending.today() LIMIT 20;
```

"Served" is Best's order today (stored `ranking_score`, debates only, not blocklisted). "On" is the order mode 2 would
serve right now: the same scores with the ranking places, the push-down and no agenda position term. "Ranking" is the
order of the ranking's places alone (NULL means the debate has no place, not that it leaves Best). The report holds
every debate in any of the three top 50s, each with its real served and if-on ranks.

`pushed_above_unranked` should be 0: it counts pushed-down debates that would still sit above an unranked debate
created within 30 days of them. If it isn't, raise `ranking_pushdown` (see Tuning).

## 3. The cutover bar (GEO-3591; Preston decides)

Cut over only when the latest `ranking_shadow_days` row shows all of:

- `rankers >= 3`;
- `recent_ranked >= 0.8 * recent_debates` (80% of the last 7 days' debates ranked);
- `all_time_top_two_ballots = all_time_top` with `all_time_top = 30` (each of the All time top 30 on 2+ ballots);
- `pushed_above_unranked = 0`;
- and the shadow report has been reviewed (Preston).

## 4. Cutover (Patrick runs steps 1 and 2; anyone checks step 3; Preston ships step 4)

In one release:

1. ```sql
   UPDATE entity_ranking_config SET ranking_places_mode = 2 WHERE id;
   ```
   (`ranking_pushdown` is 150 by default; it is config.)
2. Force the full pass.
3. Within 1 hour, Best's top 10 against the live API has changed to the ranking's head. Debates on agendas keep only
   their plain listing boost (8, fading).
4. geogenesis: flip `BEST_DEBATES_RANKING_LIVE` to `true` in `apps/web/core/blocks/ranking/best-debates.ts`, so the
   "pushes it down for everyone" copy turns on in the same release.

## Rollback (Patrick runs steps 1 and 2; Preston ships step 4)

1. ```sql
   UPDATE entity_ranking_config SET ranking_places_mode = 1 WHERE id;   -- back to shadow: agendas set places again
   ```
2. Force the full pass.
3. Within 1 hour, Best's top 10 is the agendas' order again. (`ranking_places_mode = 0` also empties the ranking's
   tables and stops the report.)
4. geogenesis: flip `BEST_DEBATES_RANKING_LIVE` back to `false`.

A rollback restores exactly the scores the agendas give: the 0124 SQL suite checks that a shadow full pass after
`on` returns every stored score to its off value.

## Tuning without a mode change

Weights, windows, slots and fades are rows of `trending.ranking_place_sources`; base, step and the push-down size are
`ranking_place_base`, `ranking_place_step` and `ranking_pushdown` in `entity_ranking_config`. Change them, then force
the full pass.

Note on the push-down: while on, a pushed-down debate loses its place and its age credit and takes −150. Against an
unranked debate up to 30 days older it starts 57.6 ahead on recency (30 × 86400 / `tau_seconds` at 45000) and can
carry a listing or topic boost (≤ 8), so it stays below while the rest of its organic lead is under 84.4 points. The
organic caps add up to about 90 at the extreme, so watch `pushed_above_unranked` and raise `ranking_pushdown` if it is
ever above 0.
