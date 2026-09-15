-- GEO-2885: a denormalised `(type_id, ranking_score DESC, entity_id DESC)` table, so a
-- type-scoped feed is an ordered index walk that stops at `first` instead of a strategy
-- that is only right for one end of the type-size range.
--
-- ADDITIVE AND UNREAD. This creates the table, its index and the maintenance hook.
-- Nothing queries it until a follow-up switches `entities_ranked_for_feed_by_type`, and
-- that switch cannot happen until the backfill has run — see the end of this header.
--
-- WHY — AND A CORRECTION TO 0082'S HEADER
--
-- 0082 left a stated ceiling: a semi-join inverts which case is cheap, so the typed feed has
-- two strategies and neither covers both ends. It then said the ceiling was safely out of
-- reach, because "the largest in EXPLORE_ENTITY_TYPES is Person at 17,467".
--
-- That is wrong. **Claim is in EXPLORE_ENTITY_TYPES — it is one of the three types Explore
-- opens with — and it has 334,808 members.** The type 0082 named as the pathological case,
-- `96f859ef`, IS Claim. Nothing is currently broken by the error, because Explore's Best sort
-- calls `entitiesRankedForFeedConnection` (the untyped walk) rather than the by-type function;
-- but the ceiling was never the comfortable distance away that 0082 claimed, and anything that
-- routes Explore through `entities_ranked_for_feed_by_type` walks straight into an 11-second
-- query. Do not rely on that paragraph.
--
-- Measured against live data on 2026-09-14, at Explore's window (`first: 66`), with the feed's
-- full predicate set applied:
--
--                                        ranked walk       semi-join (0082)   this table
--   Debate, 63 qualifying                  TIMEOUT             9 ms             4.3 ms
--   Claim, 334,808                          10 ms         11,405 ms            3.6 ms
--   Person, 17,467                       1,617 ms              --              3.5 ms
--   the 12-type whitelist                   16 ms         11,408 ms           15.6 ms
--   Explore's 3 defaults                    39 ms              --              5.0 ms
--
-- THE WALK'S TIMEOUT IS REACHABLE FROM THE UI TODAY
--
-- That first row is not hypothetical, and it is worth understanding before deciding how
-- urgent this is. The walk is cheap while it can still FIND matches and catastrophic the
-- moment it has to PROVE there are no more — at that point it scans the whole 48.9M-row
-- `entity_ranking_scores_ranking_desc_idx`. Debate-only, on live data:
--
--   LIMIT 22 (Explore's page size)        9 ms
--   LIMIT 50                             12 ms
--   LIMIT 63 (exactly how many exist) 11,245 ms
--   LIMIT 64 (one more than exists)     TIMEOUT
--
-- So page one is fine and the LAST page of any type filter narrow enough to run out is not.
-- Confirmed end to end, not just in psql: `entitiesRankedForFeedConnection` with
-- `typeIds: [Debate]` and Explore's own page-3 cursor returns
-- `{"errors":[{"message":"Unexpected error","extensions":{"code":"INTERNAL_SERVER_ERROR"}}]}`
-- after 30.04 s, which is the API's statement timeout. Any type scarce enough to exhaust
-- mid-feed does this, and Debate is the scarcest thing Explore offers.
--
-- Widening `EXPLORE_ENTITY_TYPES` is also unsafe today for the opposite reason, which is what
-- makes a strategy switch the wrong answer: no single plan is good at both ends. Denormalised,
-- both ends are an ordered walk that terminates at `first`, and the ceiling goes away.
--
-- Build cost is trivial, so a periodic full rebuild stays an option: **1,311,783 rows in
-- 12.5 s**, index 1.8 s, ANALYZE 0.13 s, measured by building it inside a rolled-back
-- transaction against live data.
--
-- THE TRAP, FOR WHOEVER WRITES THE QUERY
--
-- The natural multi-type phrasing is `WHERE type_id = ANY($1) ORDER BY ranking_score
-- DESC LIMIT $first`. It is correct and 75x slower: Postgres does one index search per
-- type, reads 56,477 rows and top-N sorts them — 251 ms. `= ANY` does not preserve the
-- index ordering. Drive one ordered scan PER TYPE with a LATERAL, each capped at the
-- limit, and it reads 701 rows in 3.3 ms:
--
--   SELECT s.* FROM unnest(type_ids) AS t(tid)
--   CROSS JOIN LATERAL (
--     SELECT entity_id, ranking_score FROM entity_type_ranking
--      WHERE type_id = t.tid ORDER BY ranking_score DESC, entity_id DESC LIMIT $first
--   ) s
--   ORDER BY s.ranking_score DESC, s.entity_id DESC LIMIT $first;
--
-- Same family as 0077's `IS NULL OR` guard and 0082's `IN`-under-`OR`: the planner will
-- not find the good plan from the natural phrasing, and the bad plan is fast enough on a
-- dev-sized table to look fine.
--
-- The feed's other predicates (blocklist, System Type, editorial exclusions, a non-empty
-- name in `space_ids`) must go INSIDE the lateral, not outside it. Applied outside, a
-- per-type `LIMIT first` can be eaten entirely by filtering and the feed silently
-- returns short. The table above was measured with all of them inside, so those numbers are
-- the real shape rather than a skeleton.
--
-- And the switch MUST de-duplicate. `entities_ranked_for_feed_by_type` gets that for free
-- today — `e.id IN (SELECT ...)` is a semi-join, so an entity carrying two of the requested
-- types comes back once. The LATERAL is not a semi-join; it is one walk per type,
-- concatenated, so that entity comes back once per matching type. With Explore defaulting to
-- News story + Debate + Claim, anything typed both Claim and News story would render twice.
-- `DISTINCT ON (entity_id)` inside, then re-sort. Both halves are pinned in
-- drizzle/tests/0084_entity_type_ranking.sql.
--
-- WHY MAINTENANCE IS A HOOK AND NOT A TRIGGER
--
-- `refresh_entity_ranking_scores` is the single choke point for score changes — it is
-- the only thing that writes `entity_ranking_scores`, and its callers are
-- scoring-service/vote-indexer (`storage.rs` and `bin/comment_sweep.rs`). Syncing the
-- type rows for exactly the ids it was handed, in the same transaction, keeps this table
-- in lockstep with NO ranking-indexer change and NO trigger on `relations`. Both looked
-- required and are not.
--
-- Freshness is inherited rather than added: the ranked feed already JOINs
-- `entity_ranking_scores`, so an entity with no score row is ALREADY invisible to the
-- feed. This table is exactly as fresh as that one.
--
-- It is safe to maintain incrementally at all because the score is time-invariant —
-- 0073's `created_at / tau` moves only when votes, type weight or structure change.
-- There is no decay pass to chase.
--
-- THE RESIDUAL GAP IS TYPE CHANGES, AND IT PREDATES THIS TABLE
--
-- Nothing calls `refresh_entity_ranking_scores` when a Types relation is added or
-- removed, so an entity that gains a type keeps a stale row set until something else
-- re-scores it. That is NOT a new defect: `ranking_score` already contains
-- `log(type_weight)`, so a type change already fails to re-score today. The DELETE below
-- means the sync is a true reconcile rather than an append, so the backfill script
-- doubles as the periodic reconciler.
--
-- BACKFILL, THEN SWITCH — IN THAT ORDER
--
-- The table is empty when this lands, so anything reading it would return nothing.
-- `api`'s initContainer runs `db:migrate`, so the populate is deliberately NOT inline
-- (0073's reason: a long statement here stalls every deploy). Run
-- `api/scripts/backfill-entity-type-ranking.sh`, confirm the row count, and only then
-- switch the function. A full rebuild measured 1,311,665 rows in 7.3 s, index 4.0 s,
-- ANALYZE 0.14 s.

CREATE TABLE IF NOT EXISTS "entity_type_ranking" (
  "type_id" uuid NOT NULL,
  "entity_id" uuid NOT NULL,
  -- Denormalised from entity_ranking_scores. Present so the index below is a covering
  -- ordered walk; without it every candidate needs a lookup and the ordering is lost.
  "ranking_score" numeric NOT NULL,
  PRIMARY KEY ("type_id", "entity_id")
);
--> statement-breakpoint
-- The point of the table. `entity_id` breaks ties in the same direction as
-- `entity_ranking_scores_ranking_desc_idx` so cursor pagination stays a total order and
-- the two paths agree on row order.
CREATE INDEX IF NOT EXISTS "entity_type_ranking_type_score_desc_idx"
  ON "entity_type_ranking" ("type_id", "ranking_score" DESC, "entity_id" DESC);
--> statement-breakpoint
-- Lets the per-entity reconcile below be a range scan rather than a seq scan; the
-- primary key is useless for it because entity_id is the trailing column.
CREATE INDEX IF NOT EXISTS "entity_type_ranking_entity_idx"
  ON "entity_type_ranking" ("entity_id");
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Unchanged from 0083 except for the two statements at the end, which reconcile
-- `entity_type_ranking` for exactly the ids this call was given, inside the same
-- transaction as the score write.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.refresh_entity_ranking_scores(entity_ids uuid[])
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  cfg          entity_ranking_config;
  affected     integer;
  types_prop   constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  reply_to     constant uuid := '310d4a24-0e5b-451c-b215-1bfce40d0fe6';
BEGIN
  SELECT * INTO cfg FROM entity_ranking_config WHERE id;

  WITH target AS (
    SELECT e.id, NULLIF(e.created_at, '')::bigint AS created_epoch
    FROM entities e
    WHERE e.id = ANY(entity_ids)
  ),
  votes AS (
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 0
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  stance AS (
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 1
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  comments AS (
    -- DISTINCT space_id, not count(*): the space a reply was authored from is who wrote
    -- it (`notification-indexer` resolves `commenter_space_id` from the edit's space the
    -- same way), and a comment is free to make, so raw volume is the one input here an
    -- individual can run up on their own.
    --
    -- Direct replies to the entity only. A reply to a *comment* points at that comment,
    -- so thread depth does not inflate the parent — engagement with the entity is what
    -- this is measuring.
    SELECT r.to_entity_id AS id,
           count(DISTINCT r.space_id)::bigint AS commenter_count
    FROM relations r
    WHERE r.type_id = reply_to AND r.to_entity_id = ANY(entity_ids)
    GROUP BY r.to_entity_id
  ),
  props AS (
    SELECT v.entity_id AS id, count(DISTINCT v.property_id)::integer AS property_count
    FROM values v WHERE v.entity_id = ANY(entity_ids) GROUP BY v.entity_id
  ),
  rels AS (
    SELECT r.from_entity_id AS id, count(*)::integer AS relation_count
    FROM relations r
    WHERE r.from_entity_id = ANY(entity_ids) AND r.is_system = false
    GROUP BY r.from_entity_id
  ),
  weights AS (
    SELECT r.from_entity_id AS id, MAX(w.weight) AS type_weight
    FROM relations r
    JOIN entity_type_weights w ON w.type_id = r.to_entity_id
    WHERE r.from_entity_id = ANY(entity_ids) AND r.type_id = types_prop
    GROUP BY r.from_entity_id
  ),
  computed AS (
    SELECT t.id,
           public.wilson_lower_bound(COALESCE(v.positive, 0), COALESCE(v.negative, 0),
                                     cfg.wilson_z, cfg.prior_positive, cfg.prior_negative) AS quality_score,
           public.entity_intrinsic_score(COALESCE(p.property_count, 0), COALESCE(rl.relation_count, 0),
                                          cfg.intrinsic_cap, cfg.intrinsic_property_target,
                                          cfg.intrinsic_relation_target) AS intrinsic_score,
           -- Participation is engagement VOLUME, on whichever axis an entity can carry.
           -- Curation (`v`) joins stance (`s`) here; see 0083's header for why.
           public.entity_participation_score(COALESCE(s.positive, 0) + COALESCE(s.negative, 0)
                                              + COALESCE(v.positive, 0) + COALESCE(v.negative, 0),
                                              cfg.participation_weight, cfg.participation_cap)
             AS participation_score,
           public.entity_participation_score(COALESCE(cm.commenter_count, 0),
                                              cfg.comment_weight, cfg.comment_cap)
             AS comment_score,
           COALESCE(w.type_weight, 1.0) AS type_weight,
           COALESCE(v.positive, 0) AS positive,
           COALESCE(v.negative, 0) AS negative,
           COALESCE(s.positive, 0) AS stance_positive,
           COALESCE(s.negative, 0) AS stance_negative,
           COALESCE(cm.commenter_count, 0) AS commenter_count,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v    ON v.id = t.id
    LEFT JOIN stance s   ON s.id = t.id
    LEFT JOIN comments cm ON cm.id = t.id
    LEFT JOIN props p    ON p.id = t.id
    LEFT JOIN rels rl    ON rl.id = t.id
    LEFT JOIN weights w  ON w.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, comment_score, ranking_score,
     positive, negative, stance_positive, stance_negative, commenter_count, type_weight, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                      c.participation_score, c.comment_score, c.created_epoch,
                                      cfg.tau_seconds, cfg.quality_floor),
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.commenter_count,
         c.type_weight, now()
  FROM computed c
  ON CONFLICT (entity_id) DO UPDATE SET
    quality_score       = EXCLUDED.quality_score,
    intrinsic_score     = EXCLUDED.intrinsic_score,
    participation_score = EXCLUDED.participation_score,
    comment_score       = EXCLUDED.comment_score,
    ranking_score       = EXCLUDED.ranking_score,
    positive            = EXCLUDED.positive,
    negative            = EXCLUDED.negative,
    stance_positive     = EXCLUDED.stance_positive,
    stance_negative     = EXCLUDED.stance_negative,
    commenter_count     = EXCLUDED.commenter_count,
    type_weight         = EXCLUDED.type_weight,
    updated_at          = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;

  -- ---- entity_type_ranking reconcile, for these ids only --------------------
  -- DELETE first, and delete by absence rather than blanket-clearing the ids: a
  -- delete-then-insert would leave the rows missing for the rest of the transaction,
  -- and every concurrent reader of this table would see those entities vanish from
  -- their type for that window.
  DELETE FROM entity_type_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND NOT EXISTS (
      SELECT 1 FROM relations r
      WHERE r.from_entity_id = etr.entity_id
        AND r.type_id = types_prop
        AND r.to_entity_id = etr.type_id
    );

  -- DISTINCT is load-bearing: a duplicate Types relation would make ON CONFLICT raise
  -- "cannot affect row a second time". `ranking_score` is functionally determined by
  -- entity_id, so distinct-on-three is distinct-on-the-pair.
  --
  -- Reads back from entity_ranking_scores rather than recomputing, so the two tables
  -- cannot disagree even if the INSERT above is ever changed.
  INSERT INTO entity_type_ranking (type_id, entity_id, ranking_score)
  SELECT DISTINCT r.to_entity_id, r.from_entity_id, ers.ranking_score
  FROM relations r
  JOIN entity_ranking_scores ers ON ers.entity_id = r.from_entity_id
  WHERE r.from_entity_id = ANY(entity_ids)
    AND r.type_id = types_prop
  ON CONFLICT (type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score;

  -- Deliberately returns the SCORE row count, not the type row count. Callers and the
  -- existing backfill script both read this as "entities scored"; a type-row count would
  -- be a different number for the same work and would silently break their progress
  -- reporting.
  RETURN affected;
END;
$$;
