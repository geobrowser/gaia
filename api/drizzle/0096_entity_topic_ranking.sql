-- GEO-3077: a ranked feed for a set of followed topics, fast at 100+ topics.
--
-- For you is Best restricted to entities tagged (Topics property) with any topic the reader
-- follows. A topic-scoped feed today filters the untyped ranked walk by relation, the shape
-- that timed out for types before GEO-2885. So this does for topics what 0084/0085 did for
-- types: a denormalised table walked in score order once per topic by a LATERAL and merged,
-- with 0084's two traps handled the same way (`= ANY` loses the ordering; the LATERAL does
-- not de-duplicate).
--
-- KEYED BY (TOPIC, TYPE), NOT TOPIC ALONE. Explore always sends its types, and filtering a
-- topic-only walk by type makes each walk read far past `first` whenever a topic holds other
-- types. With the type in the key, every (topic, type) pair is its own ordered walk that stops
-- at the cap. Measured on the live DB 2026-09-29, with Explore's three default types, first
-- page (66) and page 3 (198):
--
--                                  topic-only table    this table
--   10 topics                            51 ms            1.6 ms
--   100 topics (the 100 largest)        194 ms           17.9 ms
--   100 topics, page 3                     --           50.2 ms
--   today's typed Best, same types      4.9 ms (for comparison)
--
-- It is barely bigger (424,701 rows against 402,549): few entities carry two types.
--
-- ROWS ARE PREFILTERED to entities that can ever be in a feed: named somewhere, not a System
-- entity, and carrying no editorially excluded type (`entity_topic_ranking_eligible`). The
-- single largest topic has 32,704 tagged entities and none of them is named, so without this
-- every walk of it read all 32,704 to return nothing, 230 ms per request. The read therefore
-- skips those checks, which is most of the difference above. Only the blocklist (0 rows today,
-- and meant to act at once) stays in the read.
--
-- FRESHNESS. `refresh_entity_ranking_scores` is the choke point for score changes and now
-- reconciles topic rows for the ids it scores, as it does type rows, so a scored entity's
-- topics are as fresh as its score. It cannot see a tag, name or type added to an entity whose
-- score does not change, or a change to entity_type_exclusions. `reconcile_entity_topic_ranking()`
-- repairs everything in one pass (24s as the backfill, 20s with nothing to change, so the
-- CronJob raises the app role's 30s statement_timeout for it) and runs
-- hourly from ranking-indexer's `topic_ranking_reconcile` CronJob, the same window new entities
-- are scored in (vote-indexer's new_entity_sweep). The table is created empty, and nothing reads
-- it until that first reconcile has run.
--
-- WEIGHTS are additive, in ranking-score units (one day of recency is 0.864). With no weights,
-- or equal ones, the order is exactly Best's. An entity matching several requested topics
-- appears once, at its best weighted score.

CREATE TABLE IF NOT EXISTS "entity_topic_ranking" (
  "topic_id" uuid NOT NULL,
  "type_id" uuid NOT NULL,
  "entity_id" uuid NOT NULL,
  -- Denormalised from entity_ranking_scores, so the walks below are covering.
  "ranking_score" numeric NOT NULL,
  PRIMARY KEY ("topic_id", "type_id", "entity_id")
);
--> statement-breakpoint

-- The typed walk: one ordered scan per (topic, type).
CREATE INDEX IF NOT EXISTS "entity_topic_ranking_typed_idx"
  ON "entity_topic_ranking" ("topic_id", "type_id", "ranking_score" DESC, "entity_id" DESC);
--> statement-breakpoint

-- The untyped walk. An entity with two types has two rows here, adjacent in this order (same
-- score, same id), which is what lets the read de-duplicate inside the walk.
CREATE INDEX IF NOT EXISTS "entity_topic_ranking_untyped_idx"
  ON "entity_topic_ranking" ("topic_id", "ranking_score" DESC, "entity_id" DESC);
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "entity_topic_ranking_entity_idx"
  ON "entity_topic_ranking" ("entity_id");
--> statement-breakpoint

COMMENT ON TABLE "entity_topic_ranking" IS E'@omit';
--> statement-breakpoint

-- Whether an entity can ever appear in a topic feed: named somewhere, not a System entity, and
-- no editorially excluded type. The one definition the hook, the reconcile and the tests share.
CREATE OR REPLACE FUNCTION public.entity_topic_ranking_eligible(entity uuid)
RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT EXISTS (
           SELECT 1 FROM public.values v
           WHERE v.entity_id = entity
             AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid  -- Name
             AND v.text IS NOT NULL
             AND length(trim(v.text)) > 0
         )
     AND NOT EXISTS (
           SELECT 1 FROM public.relations r
           WHERE r.from_entity_id = entity
             AND r.type_id = '88b3d6ad-288c-529c-a212-0e1c24819185'::uuid  -- System Type
         )
     AND NOT EXISTS (
           SELECT 1 FROM public.relations r
           JOIN public.entity_type_exclusions x ON x.type_id = r.to_entity_id
           WHERE r.from_entity_id = entity
             AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
         );
$$;
--> statement-breakpoint

COMMENT ON FUNCTION public.entity_topic_ranking_eligible(uuid) IS E'@omit';
--> statement-breakpoint

-- The score choke point as 0084 defined it, with the topic reconcile added after the type one.
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

  -- ---- entity_topic_ranking reconcile, for these ids only (0096) -------------
  -- Same shape as the type block above: delete by absence first, then upsert from
  -- entity_ranking_scores. One row per (topic, type, entity) the entity has, and only
  -- for entities `entity_topic_ranking_eligible` admits.
  DELETE FROM entity_topic_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND (
      NOT EXISTS (
        SELECT 1 FROM relations r
        WHERE r.from_entity_id = etr.entity_id AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid AND r.to_entity_id = etr.topic_id
      )
      OR NOT EXISTS (
        SELECT 1 FROM relations r
        WHERE r.from_entity_id = etr.entity_id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND r.to_entity_id = etr.type_id
      )
      OR NOT public.entity_topic_ranking_eligible(etr.entity_id)
    );

  INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score)
  SELECT DISTINCT tp.to_entity_id, ty.to_entity_id, tp.from_entity_id, ers.ranking_score
  FROM relations tp
  JOIN relations ty ON ty.from_entity_id = tp.from_entity_id AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
  JOIN entity_ranking_scores ers ON ers.entity_id = tp.from_entity_id
  WHERE tp.from_entity_id = ANY(entity_ids)
    AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
    AND public.entity_topic_ranking_eligible(tp.from_entity_id)
  ON CONFLICT (topic_id, type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score;

  -- Deliberately returns the SCORE row count, not the type row count. Callers and the
  -- existing backfill script both read this as "entities scored"; a type-row count would
  -- be a different number for the same work and would silently break their progress
  -- reporting.
  RETURN affected;
END;
$$;
--> statement-breakpoint

-- Repair every topic row in one pass: drop rows whose tag, type, score or eligibility is gone,
-- add missing rows and fix drifted scores. Returns the number of rows changed. Runs hourly, and
-- is the backfill.
CREATE OR REPLACE FUNCTION public.reconcile_entity_topic_ranking()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  removed integer;
  ineligible integer;
  upserted integer;
BEGIN
  -- Eligibility is judged once per distinct tagged entity, not once per (topic, type) row:
  -- the per-row version took 28s with nothing to change.
  CREATE TEMP TABLE IF NOT EXISTS reconcile_eligible (entity_id uuid PRIMARY KEY) ON COMMIT DROP;
  TRUNCATE reconcile_eligible;
  INSERT INTO reconcile_eligible (entity_id)
  SELECT c.entity_id
  FROM (SELECT DISTINCT r.from_entity_id AS entity_id FROM relations r WHERE r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid) c
  WHERE EXISTS (SELECT 1 FROM entity_ranking_scores ers WHERE ers.entity_id = c.entity_id)
    AND public.entity_topic_ranking_eligible(c.entity_id);
  ANALYZE reconcile_eligible;

  DELETE FROM entity_topic_ranking etr
  WHERE NOT EXISTS (SELECT 1 FROM reconcile_eligible re WHERE re.entity_id = etr.entity_id);
  GET DIAGNOSTICS ineligible = ROW_COUNT;

  DELETE FROM entity_topic_ranking etr
  WHERE NOT EXISTS (
          SELECT 1 FROM relations r
          WHERE r.from_entity_id = etr.entity_id AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid AND r.to_entity_id = etr.topic_id
        )
     OR NOT EXISTS (
          SELECT 1 FROM relations r
          WHERE r.from_entity_id = etr.entity_id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND r.to_entity_id = etr.type_id
        );
  GET DIAGNOSTICS removed = ROW_COUNT;

  INSERT INTO entity_topic_ranking AS etr (topic_id, type_id, entity_id, ranking_score)
  SELECT DISTINCT tp.to_entity_id, ty.to_entity_id, tp.from_entity_id, ers.ranking_score
  FROM reconcile_eligible re
  JOIN relations tp ON tp.from_entity_id = re.entity_id AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
  JOIN relations ty ON ty.from_entity_id = re.entity_id AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
  JOIN entity_ranking_scores ers ON ers.entity_id = re.entity_id
  ON CONFLICT (topic_id, type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score
  WHERE etr.ranking_score IS DISTINCT FROM EXCLUDED.ranking_score;
  GET DIAGNOSTICS upserted = ROW_COUNT;

  RETURN ineligible + removed + upserted;
END;
$$;
--> statement-breakpoint

COMMENT ON FUNCTION public.reconcile_entity_topic_ranking() IS E'@omit';
--> statement-breakpoint

-- The For you read (GraphQL `entitiesRankedForTopics` / `entitiesRankedForTopicsConnection`).
--
-- `topic_weights`, when given, must be one per topic. `max_per_topic` caps each walk and must be
-- at least offset + first, exactly as `max_per_type` is for `entities_ranked_for_feed_by_type`;
-- a smaller cap returns short pages. A walk is ordered by the unweighted score and a topic's
-- weight is constant within it, so the union of the per-walk top N always contains the weighted
-- global top N.
CREATE OR REPLACE FUNCTION public.entities_ranked_for_topics(
  topic_ids uuid[],
  topic_weights double precision[] DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL,
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL,
  space_ids uuid[] DEFAULT NULL,
  max_per_topic integer DEFAULT NULL
)
RETURNS SETOF public.entities
LANGUAGE plpgsql STABLE PARALLEL SAFE AS $$
DECLARE
  weights double precision[];
BEGIN
  IF topic_weights IS NOT NULL
     AND coalesce(array_length(topic_weights, 1), 0) <> coalesce(array_length(topic_ids, 1), 0) THEN
    RAISE EXCEPTION 'topicWeights must have one weight per topic (got % for % topics)',
      coalesce(array_length(topic_weights, 1), 0), coalesce(array_length(topic_ids, 1), 0)
      USING ERRCODE = '22023';
  END IF;
  weights := coalesce(topic_weights, array_fill(0::double precision, ARRAY[coalesce(array_length(topic_ids, 1), 0)]));

  -- Two statements rather than one with `type_ids IS NULL OR …`: the typed path walks a
  -- different index, and one plan cannot serve both.
  IF type_ids IS NOT NULL THEN
    RETURN QUERY
    SELECT e.*
    FROM (
     -- Cut to the page before joining `entities`, so only rows that can be returned are
     -- fetched: joining every walk's rows first cost 50 ms of heap reads at 100 topics.
     SELECT u.entity_id, u.weighted FROM (
      SELECT DISTINCT ON (s.entity_id) s.entity_id, s.weighted
      FROM unnest(topic_ids, weights) AS t(tid, weight)
      CROSS JOIN unnest(type_ids) AS y(yid)
      CROSS JOIN LATERAL (
        SELECT etr.entity_id, etr.ranking_score + coalesce(t.weight, 0)::numeric AS weighted
        FROM public.entity_topic_ranking etr
        WHERE etr.topic_id = t.tid
          AND etr.type_id = y.yid
          AND (min_ranking_score IS NULL OR etr.ranking_score >= min_ranking_score)
          AND (created_after IS NULL AND created_before IS NULL OR EXISTS (
            SELECT 1 FROM public.entities e2
            WHERE e2.id = etr.entity_id
              AND (created_after  IS NULL OR e2.created_at >  created_after)
              AND (created_before IS NULL OR e2.created_at <= created_before)
          ))
          -- Named in a requested space. Without a space filter the prefilter already
          -- guarantees a name, so there is nothing to check.
          AND (space_ids IS NULL OR EXISTS (
            SELECT 1 FROM public.values v
            WHERE v.entity_id = etr.entity_id
              AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
              AND v.text IS NOT NULL
              AND length(trim(v.text)) > 0
              AND v.space_id = ANY(space_ids)
          ))
          AND NOT EXISTS (
            SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id
          )
        ORDER BY etr.ranking_score DESC, etr.entity_id DESC
        LIMIT max_per_topic
      ) s
      ORDER BY s.entity_id, s.weighted DESC
     ) u
     ORDER BY u.weighted DESC, u.entity_id DESC
     -- The contract is offset + first <= max_per_topic, so nothing past it can be on the page.
     LIMIT max_per_topic
    ) d
    JOIN public.entities e ON e.id = d.entity_id
    ORDER BY d.weighted DESC, d.entity_id DESC;
  ELSE
    RETURN QUERY
    SELECT e.*
    FROM (
     -- Cut to the page before joining `entities`, so only rows that can be returned are
     -- fetched: joining every walk's rows first cost 50 ms of heap reads at 100 topics.
     SELECT u.entity_id, u.weighted FROM (
      SELECT DISTINCT ON (s.entity_id) s.entity_id, s.weighted
      FROM unnest(topic_ids, weights) AS t(tid, weight)
      CROSS JOIN LATERAL (
        -- DISTINCT inside the walk: an entity's rows for its several types are adjacent in
        -- this index order, so it costs nothing and keeps the cap counting entities, not rows.
        SELECT DISTINCT etr.ranking_score, etr.entity_id,
               etr.ranking_score + coalesce(t.weight, 0)::numeric AS weighted
        FROM public.entity_topic_ranking etr
        WHERE etr.topic_id = t.tid
          AND (min_ranking_score IS NULL OR etr.ranking_score >= min_ranking_score)
          AND (created_after IS NULL AND created_before IS NULL OR EXISTS (
            SELECT 1 FROM public.entities e2
            WHERE e2.id = etr.entity_id
              AND (created_after  IS NULL OR e2.created_at >  created_after)
              AND (created_before IS NULL OR e2.created_at <= created_before)
          ))
          AND (space_ids IS NULL OR EXISTS (
            SELECT 1 FROM public.values v
            WHERE v.entity_id = etr.entity_id
              AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
              AND v.text IS NOT NULL
              AND length(trim(v.text)) > 0
              AND v.space_id = ANY(space_ids)
          ))
          AND NOT EXISTS (
            SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id
          )
        ORDER BY etr.ranking_score DESC, etr.entity_id DESC
        LIMIT max_per_topic
      ) s
      ORDER BY s.entity_id, s.weighted DESC
     ) u
     ORDER BY u.weighted DESC, u.entity_id DESC
     -- The contract is offset + first <= max_per_topic, so nothing past it can be on the page.
     LIMIT max_per_topic
    ) d
    JOIN public.entities e ON e.id = d.entity_id
    ORDER BY d.weighted DESC, d.entity_id DESC;
  END IF;
END;
$$;
--> statement-breakpoint

-- Which of `topic_ids` an entity is tagged with (GraphQL: `matchedTopicIds(topicIds: …)` on
-- Entity), so the web app can label an item "because you follow X" and stop one topic taking
-- over a page. Read from the relations themselves, the source of truth.
CREATE OR REPLACE FUNCTION public.entities_matched_topic_ids(e public.entities, topic_ids uuid[])
RETURNS uuid[]
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT coalesce(array_agg(DISTINCT r.to_entity_id ORDER BY r.to_entity_id), ARRAY[]::uuid[])
  FROM public.relations r
  WHERE r.from_entity_id = e.id
    AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
    AND r.to_entity_id = ANY(topic_ids);
$$;
