-- GEO-3092: what a topic page needs from the fast topic feed, beyond For you.
--
-- A topic page's Best feed downloads everything tagged with the topic and sorts it in the app,
-- 1.83s on a first load of "Russia-Ukraine war" against 0.44s for Explore (measured 2026-09-29).
-- The fast feed from 0096/0097 answers Best for "any of these topics". A topic page also has:
--
--   * a New sort (newest first): rows now carry `created_epoch`, with newest-first indexes, so it
--     is the same bounded walk in a different order. Weights do not apply to New.
--   * extra topics that narrow the page with "all of these topics" (`match_all`): only the
--     rarest requested topic is walked, and every row must carry the others too, so the walk is
--     as short as the smallest topic allows.
--   * a composition strip counting feed entities per type (`topic_feed_type_counts`), today
--     computed from the same full download.
--
-- The read switches to dynamic SQL so that each order plans against its own index; the
-- predicates are unchanged from 0097. The reconcile fills `created_epoch` for existing rows the
-- first time it runs after this migration, which is the backfill.

ALTER TABLE "entity_topic_ranking" ADD COLUMN IF NOT EXISTS "created_epoch" bigint;
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "entity_topic_ranking_typed_new_idx"
  ON "entity_topic_ranking" ("topic_id", "type_id", "created_epoch" DESC NULLS LAST, "entity_id" DESC);
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "entity_topic_ranking_untyped_new_idx"
  ON "entity_topic_ranking" ("topic_id", "created_epoch" DESC NULLS LAST, "entity_id" DESC);
--> statement-breakpoint

-- The score choke point as 0097 left it, now also writing created_epoch.
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
  -- entity_ranking_scores. One row per (topic, kind, entity), kinds being the entity's types
  -- plus the Debate tag for a tagged claim (0097), and only for eligible entities.
  DELETE FROM entity_topic_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND (
      NOT EXISTS (
        SELECT 1 FROM relations r
        WHERE r.from_entity_id = etr.entity_id AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid AND r.to_entity_id = etr.topic_id
      )
      OR etr.type_id NOT IN (SELECT public.entity_topic_ranking_kinds(etr.entity_id))
      OR NOT public.entity_topic_ranking_eligible(etr.entity_id)
    );

  INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score, created_epoch)
  SELECT DISTINCT tp.to_entity_id, k.kind, tp.from_entity_id, ers.ranking_score, CASE WHEN ce.created_at ~ '^[0-9]+$' THEN ce.created_at::bigint END
  FROM relations tp
  CROSS JOIN LATERAL public.entity_topic_ranking_kinds(tp.from_entity_id) AS k(kind)
  JOIN entity_ranking_scores ers ON ers.entity_id = tp.from_entity_id
  JOIN entities ce ON ce.id = tp.from_entity_id
  WHERE tp.from_entity_id = ANY(entity_ids)
    AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
    AND public.entity_topic_ranking_eligible(tp.from_entity_id)
  ON CONFLICT (topic_id, type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score,
    created_epoch = EXCLUDED.created_epoch;

  -- Deliberately returns the SCORE row count, not the type row count. Callers and the
  -- existing backfill script both read this as "entities scored"; a type-row count would
  -- be a different number for the same work and would silently break their progress
  -- reporting.
  RETURN affected;
END;
$$;
--> statement-breakpoint

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
     OR etr.type_id NOT IN (SELECT public.entity_topic_ranking_kinds(etr.entity_id));
  GET DIAGNOSTICS removed = ROW_COUNT;

  INSERT INTO entity_topic_ranking AS etr (topic_id, type_id, entity_id, ranking_score, created_epoch)
  SELECT DISTINCT tp.to_entity_id, k.kind, tp.from_entity_id, ers.ranking_score, CASE WHEN ce.created_at ~ '^[0-9]+$' THEN ce.created_at::bigint END
  FROM reconcile_eligible re
  JOIN relations tp ON tp.from_entity_id = re.entity_id AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
  CROSS JOIN LATERAL public.entity_topic_ranking_kinds(re.entity_id) AS k(kind)
  JOIN entity_ranking_scores ers ON ers.entity_id = re.entity_id
  JOIN entities ce ON ce.id = re.entity_id
  ON CONFLICT (topic_id, type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score,
    created_epoch = EXCLUDED.created_epoch
  WHERE etr.ranking_score IS DISTINCT FROM EXCLUDED.ranking_score
     OR etr.created_epoch IS DISTINCT FROM EXCLUDED.created_epoch;
  GET DIAGNOSTICS upserted = ROW_COUNT;

  RETURN ineligible + removed + upserted;
END;
$$;
--> statement-breakpoint

DROP FUNCTION IF EXISTS public.entities_ranked_for_topics(uuid[], double precision[], uuid[], numeric, text, text, uuid[], integer, boolean);
--> statement-breakpoint

CREATE FUNCTION public.entities_ranked_for_topics(
  topic_ids uuid[],
  topic_weights double precision[] DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL,
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL,
  space_ids uuid[] DEFAULT NULL,
  max_per_topic integer DEFAULT NULL,
  debate_tagged_claims boolean DEFAULT false,
  sort_by text DEFAULT 'best',
  match_all boolean DEFAULT false
)
RETURNS SETOF public.entities
LANGUAGE plpgsql STABLE PARALLEL SAFE AS $$
DECLARE
  weights double precision[];
  walk_types uuid[];
  walk_topics uuid[];
  others uuid[] := ARRAY[]::uuid[];
  rarest uuid;
  order_col text;
  weighted text;
BEGIN
  IF topic_weights IS NOT NULL
     AND coalesce(array_length(topic_weights, 1), 0) <> coalesce(array_length(topic_ids, 1), 0) THEN
    RAISE EXCEPTION 'topicWeights must have one weight per topic (got % for % topics)',
      coalesce(array_length(topic_weights, 1), 0), coalesce(array_length(topic_ids, 1), 0)
      USING ERRCODE = '22023';
  END IF;
  IF sort_by IS NULL OR sort_by NOT IN ('best', 'new') THEN
    RAISE EXCEPTION 'sortBy must be best or new (got %)', sort_by USING ERRCODE = '22023';
  END IF;

  walk_types := CASE WHEN debate_tagged_claims AND type_ids IS NOT NULL
                     THEN array_replace(type_ids, '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid, '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid)
                     ELSE type_ids END;

  -- "All of these topics" (a topic page narrowed by extra topics): walk only the rarest
  -- requested topic and require the rest on each row, so the walk is as short as the smallest
  -- topic allows. Weights have nothing to separate when every entity carries every topic.
  IF match_all AND coalesce(array_length(topic_ids, 1), 0) > 1 THEN
    SELECT t.tid INTO rarest
    FROM unnest(topic_ids) AS t(tid)
    ORDER BY (SELECT count(*) FROM public.entity_topic_ranking x WHERE x.topic_id = t.tid), t.tid
    LIMIT 1;
    walk_topics := ARRAY[rarest];
    others := array_remove(topic_ids, rarest);
    weights := ARRAY[0::double precision];
  ELSE
    walk_topics := topic_ids;
    weights := coalesce(topic_weights, array_fill(0::double precision, ARRAY[coalesce(array_length(topic_ids, 1), 0)]));
  END IF;

  -- New walks the created_epoch indexes and ignores weights; Best walks the score indexes.
  -- Dynamic so each order's statement plans against its own index.
  order_col := CASE WHEN sort_by = 'new' THEN 'etr.created_epoch' ELSE 'etr.ranking_score' END;
  weighted := CASE WHEN sort_by = 'new' THEN 'etr.created_epoch::numeric'
                   ELSE 'etr.ranking_score + coalesce(t.weight, 0)::numeric' END;

  IF walk_types IS NOT NULL THEN
    RETURN QUERY EXECUTE format($q$
    SELECT e.*
    FROM (
     SELECT u.entity_id, u.weighted FROM (
      SELECT DISTINCT ON (s.entity_id) s.entity_id, s.weighted
      FROM unnest($1::uuid[], $2::double precision[]) AS t(tid, weight)
      CROSS JOIN unnest($3::uuid[]) AS y(yid)
      CROSS JOIN LATERAL (
        SELECT etr.entity_id, %2$s AS weighted
        FROM public.entity_topic_ranking etr
        WHERE etr.topic_id = t.tid
          AND etr.type_id = y.yid
          AND ($4::numeric IS NULL OR etr.ranking_score >= $4)
          AND ($5::text IS NULL AND $6::text IS NULL OR EXISTS (
            SELECT 1 FROM public.entities e2
            WHERE e2.id = etr.entity_id
              AND ($5 IS NULL OR e2.created_at >  $5)
              AND ($6 IS NULL OR e2.created_at <= $6)
          ))
          AND ($7::uuid[] IS NULL OR EXISTS (
            SELECT 1 FROM public.values v
            WHERE v.entity_id = etr.entity_id
              AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
              AND v.text IS NOT NULL
              AND length(trim(v.text)) > 0
              AND v.space_id = ANY($7)
          ))
          AND NOT EXISTS (
            SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id
          )
          -- "All of these topics": every other requested topic is on the entity too.
          AND NOT EXISTS (
            SELECT 1 FROM unnest($10::uuid[]) AS o(tid)
            WHERE NOT EXISTS (
              SELECT 1 FROM public.entity_topic_ranking x
              WHERE x.entity_id = etr.entity_id AND x.topic_id = o.tid
            )
          )
          AND (y.yid <> '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid OR $7::uuid[] IS NULL OR EXISTS (
            SELECT 1 FROM public.relations tg
            WHERE tg.from_entity_id = etr.entity_id
              AND tg.type_id = '25709034-1ba5-406f-94e4-d4af90042fba'::uuid
              AND tg.to_entity_id = '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid
              AND tg.space_id = ANY($7)
          ))
        ORDER BY %1$s DESC NULLS LAST, etr.entity_id DESC
        LIMIT $8
      ) s
      ORDER BY s.entity_id, s.weighted DESC NULLS LAST
     ) u
     ORDER BY u.weighted DESC NULLS LAST, u.entity_id DESC
     LIMIT $8
    ) d
    JOIN public.entities e ON e.id = d.entity_id
    ORDER BY d.weighted DESC NULLS LAST, d.entity_id DESC
    $q$, order_col, weighted)
    USING walk_topics, weights, walk_types, min_ranking_score, created_after, created_before,
          space_ids, max_per_topic, debate_tagged_claims, others;
  ELSE
    RETURN QUERY EXECUTE format($q$
    SELECT e.*
    FROM (
     SELECT u.entity_id, u.weighted FROM (
      SELECT DISTINCT ON (s.entity_id) s.entity_id, s.weighted
      FROM unnest($1::uuid[], $2::double precision[]) AS t(tid, weight)
      CROSS JOIN LATERAL (
        -- DISTINCT inside the walk: an entity's rows for its several kinds are adjacent in
        -- either order, so the cap counts entities, not rows.
        SELECT DISTINCT %1$s AS ord, etr.entity_id, %2$s AS weighted
        FROM public.entity_topic_ranking etr
        WHERE etr.topic_id = t.tid
          AND ($4::numeric IS NULL OR etr.ranking_score >= $4)
          AND ($5::text IS NULL AND $6::text IS NULL OR EXISTS (
            SELECT 1 FROM public.entities e2
            WHERE e2.id = etr.entity_id
              AND ($5 IS NULL OR e2.created_at >  $5)
              AND ($6 IS NULL OR e2.created_at <= $6)
          ))
          AND ($7::uuid[] IS NULL OR EXISTS (
            SELECT 1 FROM public.values v
            WHERE v.entity_id = etr.entity_id
              AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
              AND v.text IS NOT NULL
              AND length(trim(v.text)) > 0
              AND v.space_id = ANY($7)
          ))
          AND NOT EXISTS (
            SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id
          )
          -- "All of these topics": every other requested topic is on the entity too.
          AND NOT EXISTS (
            SELECT 1 FROM unnest($10::uuid[]) AS o(tid)
            WHERE NOT EXISTS (
              SELECT 1 FROM public.entity_topic_ranking x
              WHERE x.entity_id = etr.entity_id AND x.topic_id = o.tid
            )
          )
          AND (NOT $9 OR NOT EXISTS (
            SELECT 1 FROM public.entity_topic_ranking c
            WHERE c.topic_id = etr.topic_id AND c.entity_id = etr.entity_id AND c.type_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid
          ) OR EXISTS (
            SELECT 1 FROM public.relations tg
            WHERE tg.from_entity_id = etr.entity_id
              AND tg.type_id = '25709034-1ba5-406f-94e4-d4af90042fba'::uuid
              AND tg.to_entity_id = '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid
              AND ($7::uuid[] IS NULL OR tg.space_id = ANY($7))
          ))
        ORDER BY %1$s DESC NULLS LAST, etr.entity_id DESC
        LIMIT $8
      ) s
      ORDER BY s.entity_id, s.weighted DESC NULLS LAST
     ) u
     ORDER BY u.weighted DESC NULLS LAST, u.entity_id DESC
     LIMIT $8
    ) d
    JOIN public.entities e ON e.id = d.entity_id
    ORDER BY d.weighted DESC NULLS LAST, d.entity_id DESC
    $q$, order_col, weighted)
    USING walk_topics, weights, walk_types, min_ranking_score, created_after, created_before,
          space_ids, max_per_topic, debate_tagged_claims, others;
  END IF;
END;
$$;
--> statement-breakpoint

-- The topic page's composition strip: feed entities per requested type. `match_all` defaults on,
-- because the strip describes the page as narrowed. Counts an entity once per type it carries,
-- like the strip does today.
CREATE FUNCTION public.topic_feed_type_counts(
  topic_ids uuid[],
  type_ids uuid[],
  space_ids uuid[] DEFAULT NULL,
  match_all boolean DEFAULT true,
  debate_tagged_claims boolean DEFAULT false
)
RETURNS TABLE (type_id uuid, entity_count bigint)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  WITH rarest AS (
    SELECT t.tid
    FROM unnest(topic_ids) AS t(tid)
    ORDER BY (SELECT count(*) FROM public.entity_topic_ranking x WHERE x.topic_id = t.tid), t.tid
    LIMIT 1
  ),
  kinds AS (
    SELECT y.requested,
           CASE WHEN debate_tagged_claims AND y.requested = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid THEN '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid ELSE y.requested END AS walked
    FROM unnest(type_ids) AS y(requested)
  )
  SELECT k.requested, count(DISTINCT etr.entity_id)
  FROM kinds k
  JOIN public.entity_topic_ranking etr ON etr.type_id = k.walked
  WHERE (CASE WHEN match_all
              THEN etr.topic_id = (SELECT tid FROM rarest)
                   AND NOT EXISTS (
                     SELECT 1 FROM unnest(topic_ids) AS o(tid)
                     WHERE NOT EXISTS (
                       SELECT 1 FROM public.entity_topic_ranking x
                       WHERE x.entity_id = etr.entity_id AND x.topic_id = o.tid
                     )
                   )
              ELSE etr.topic_id = ANY(topic_ids) END)
    AND (space_ids IS NULL OR EXISTS (
      SELECT 1 FROM public.values v
      WHERE v.entity_id = etr.entity_id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
        AND v.text IS NOT NULL
        AND length(trim(v.text)) > 0
        AND v.space_id = ANY(space_ids)
    ))
    AND (k.walked <> '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid OR space_ids IS NULL OR EXISTS (
      SELECT 1 FROM public.relations tg
      WHERE tg.from_entity_id = etr.entity_id
        AND tg.type_id = '25709034-1ba5-406f-94e4-d4af90042fba'::uuid
        AND tg.to_entity_id = '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid
        AND tg.space_id = ANY(space_ids)
    ))
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id)
  GROUP BY k.requested;
$$;
