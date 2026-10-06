-- GEO-3150: Best ranks topic debates. Plus GEO-3088's "Topic debates" block in the interest model.
--
-- WHAT A TOPIC DEBATE IS. A debate can be about a Topic instead of a claim: Debate → Topics
-- (806d52bc) → Topic, with Participants, and no sides. The marker used everywhere below is the one
-- the ticket names: an entity typed Debate (fd51f935) that has a Topics relation and NO Claims
-- relation (e614cce1) of its own. Claim debates are filed under their claim's topics too (the web
-- app's debate-publish-draft.ts writes the claim's topics onto the debate), so "has a Topics
-- relation" alone would match every claim debate; the absent Claims relation is what separates
-- them. Transcript blocks also carry Claims relations, but from the block entity, not the debate,
-- so they do not affect the marker.
--
-- Measured on testnet-api-v2 on 2026-10-06: 144 debates, every one with exactly one Claims
-- relation, 138 of them with Topics relations (1-14 each). No topic debate exists yet.
--
-- WHERE BEST READ A DEBATE'S MAIN CLAIM. Nowhere. A debate's Best score has always come from the
-- debate entity's own votes, comments, structure, type weight and age (0073-0098); no ranking
-- function follows its Claims relation. So nothing had to be taught to skip a missing main claim.
-- The only reader of a debate's Claims relation in gaia's ranking and personalization SQL is the
-- interest model's debate credit (0100), changed below. Topic debates are typed Debate, so they
-- were already in entity_type_ranking under Debate (Explore's debate slot and type mix) and in
-- entity_topic_ranking under their topic (For you), once scored.
--
-- THE TOPIC TERM. A topic debate gets one more additive term on top of the ordinary score:
--
--   topic_score = min(topic_cap, topic_interested_weight × ln(1 + interested)
--                              + topic_debate_weight     × ln(1 + debates_held))
--
--   interested    distinct people with a current Interested on the topic (user_votes vote_kind 3,
--                 vote_type 0 = Up; GEO-3158), each counted once whatever space they voted in, and
--                 weighted by account_vote_weight (GEO-3141), so test and junk accounts count for
--                 less or nothing. Interested is the only topic-interest signal: the old Following
--                 relations are dropped (Preston, 6 Oct) and are not read. No Interested row exists
--                 yet (GEO-3158 is not live), so this part is 0 until it is.
--   debates_held  other debates (claim or topic) filed under the topic that were created no later
--                 than this one: the debates "already held" on it.
--
-- With several topics, the topic with the largest term counts. The term is added to the ordinary
-- score, so the debate's own engagement and recency decay work exactly as for any debate.
--
-- WEIGHTS (entity_ranking_config, so retuning is a row update and a re-score, not a migration).
-- Starting values: interested 0.5, debates 0.5, cap 2.0, chosen against live data on 2026-10-06:
--   * Debates' scores without the recency term: median 7.8, interquartile range 5.2-9.4, almost all
--     of it participation (4.0 × ln(1 + votes)). One day of age is 0.86 score units (tau 100000).
--   * Debates per topic: 220 topics carry a debate; 72 carry one, the busiest 20. ln(1 + 20) × 0.5
--     = 1.52, about 1.8 days of recency, a third of the engagement IQR. A topic with one earlier
--     debate adds 0.35.
--   * Interest: no Interested vote exists yet, and the 15 Following relations it replaces (from 4
--     people, at most 1 per topic) would have given at most ln(2) × 0.5 = 0.35. Debate count and
--     the debate's own engagement do the work at first, as the ticket expects. Every Interested is
--     a vote, so its author gets an account weight from the hourly refresh_account_weights.
--   * The cap (2.0, ~2.3 days of recency) keeps the term well inside what engagement decides: a
--     topic debate nobody engages with cannot outrank an engaged one of the same age.
--
-- CLAIM DEBATES ARE UNCHANGED. Their topic_score is 0 and their ranking_score is computed by the
-- same expression as 0098's; the suite for this migration asserts it, including after the topic
-- inputs (Interested, more debates, larger weights) move.
--
-- FRESHNESS. A topic debate's own votes and comments re-score it through the existing paths; a new
-- topic debate is scored by the hourly new-entity sweep. A new Interested or debate on the topic
-- changes no score by itself, so `refresh_topic_debate_scores()` re-scores every topic
-- debate, hourly, from ranking-indexer's topic_ranking_reconcile run.
--
-- GEO-3088 "Topic debates" (the interest model, personalization.user_interest_events):
--   * Joining or finishing a topic debate credits the debated topic at the debate weight (2.0),
--     the same as a claim debate credits its claim's topics. Nothing else in the function changes.
--   * Watching or joining a topic debate adds no follow credit: debates only ever emit 'debate'.
--   * The follow source (a current Interested on the topic, and no longer the Following relation)
--     is GEO-3158's change, made in 0106_interested_vote_kind (gaia#1023), not this one's.
--   * Not interested stays claims-only: external_interest_signals is untouched.
--
-- DEPENDS ON 0106_interested_vote_kind (gaia#1023), which must merge first. Both replace
-- user_interest_events; the body below is 0106's, copied verbatim so this file is self-contained,
-- plus the one topic-debate branch. Applied after 0106, it keeps everything 0106 did.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "topic_interested_weight" numeric NOT NULL DEFAULT 0.5;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "topic_debate_weight" numeric NOT NULL DEFAULT 0.5;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "topic_cap" numeric NOT NULL DEFAULT 2.0;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_topic_terms_nonnegative') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_topic_terms_nonnegative"
      CHECK ("topic_interested_weight" >= 0 AND "topic_debate_weight" >= 0 AND "topic_cap" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- Stored for explainability, like the other terms. 0 for everything that is not a topic debate.
-- A constant default, so adding it rewrites no rows.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "topic_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint

-- The topic term for those of `entity_ids` that are topic debates, with its inputs. One row per
-- topic debate (its strongest topic); nothing for anything else. Private: it reads account weights.
CREATE OR REPLACE FUNCTION public.topic_debate_scores(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, topic_id uuid, interested double precision, debates_held integer,
               topic_score numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.topic_interested_weight, c.topic_debate_weight, c.topic_cap
    FROM public.entity_ranking_config c WHERE c.id
  ),
  debates AS (
    SELECT DISTINCT ty.from_entity_id AS id
    FROM public.relations ty
    WHERE ty.from_entity_id = ANY(entity_ids)
      AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
      AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid -- Debate
      AND NOT EXISTS (
        SELECT 1 FROM public.relations c
        WHERE c.from_entity_id = ty.from_entity_id
          AND c.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid    -- Claims
      )
  ),
  debate_topics AS (
    SELECT DISTINCT d.id, tp.to_entity_id AS topic_id,
           CASE WHEN de.created_at ~ '^[0-9]+$' THEN de.created_at::bigint END AS created_epoch
    FROM debates d
    JOIN public.relations tp ON tp.from_entity_id = d.id
                            AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid  -- Topics
    JOIN public.entities de ON de.id = d.id
  ),
  inputs AS (
    SELECT dt.id, dt.topic_id,
           -- Current Interested on the topic (GEO-3158), one per person: user_votes is keyed by
           -- space too, so the same person can hold it in several.
           (SELECT coalesce(sum(public.account_vote_weight(f.user_id)), 0)::double precision
            FROM (
              SELECT DISTINCT v.user_id
              FROM public.user_votes v
              WHERE v.object_id = dt.topic_id AND v.object_type = 0
                AND v.vote_kind = 3 AND v.vote_type = 0
            ) f) AS interested,
           (SELECT count(DISTINCT o.from_entity_id)::integer
            FROM public.relations o
            JOIN public.entities oe ON oe.id = o.from_entity_id
            WHERE o.to_entity_id = dt.topic_id
              AND o.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
              AND o.from_entity_id <> dt.id
              AND oe.created_at ~ '^[0-9]+$'
              AND oe.created_at::bigint <= dt.created_epoch
              AND EXISTS (
                SELECT 1 FROM public.relations oty
                WHERE oty.from_entity_id = o.from_entity_id
                  AND oty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                  AND oty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid
              )) AS debates_held
    FROM debate_topics dt
  ),
  scored AS (
    SELECT i.*,
           LEAST(cfg.topic_cap,
                 cfg.topic_interested_weight * ln(1 + GREATEST(i.interested, 0)::numeric)
               + cfg.topic_debate_weight * ln(1 + GREATEST(i.debates_held, 0)::numeric)) AS topic_score
    FROM inputs i CROSS JOIN cfg
  )
  SELECT DISTINCT ON (s.id) s.id, s.topic_id, s.interested, s.debates_held, s.topic_score
  FROM scored s
  ORDER BY s.id, s.topic_score DESC, s.topic_id
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.topic_debate_scores(uuid[]) IS E'@omit';
--> statement-breakpoint

-- The score choke point as 0098 left it, plus the topic term. Everything else is unchanged.
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
    -- DISTINCT space_id: the space a reply was authored from is who wrote it. Direct replies only.
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
  -- GEO-3150: rows only for topic debates; everything else gets 0 below.
  topic AS (
    SELECT tds.entity_id AS id, tds.topic_score
    FROM public.topic_debate_scores(entity_ids) tds
  ),
  computed AS (
    SELECT t.id,
           public.wilson_lower_bound(COALESCE(v.positive, 0), COALESCE(v.negative, 0),
                                     cfg.wilson_z, cfg.prior_positive, cfg.prior_negative) AS quality_score,
           public.entity_intrinsic_score(COALESCE(p.property_count, 0), COALESCE(rl.relation_count, 0),
                                          cfg.intrinsic_cap, cfg.intrinsic_property_target,
                                          cfg.intrinsic_relation_target) AS intrinsic_score,
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
           tp.topic_score,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v    ON v.id = t.id
    LEFT JOIN stance s   ON s.id = t.id
    LEFT JOIN comments cm ON cm.id = t.id
    LEFT JOIN props p    ON p.id = t.id
    LEFT JOIN rels rl    ON rl.id = t.id
    LEFT JOIN weights w  ON w.id = t.id
    LEFT JOIN topic tp   ON tp.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, comment_score, ranking_score,
     positive, negative, stance_positive, stance_negative, commenter_count, type_weight, topic_score,
     updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         -- Not a topic debate: exactly 0098's expression, with nothing added.
         CASE WHEN c.topic_score IS NULL
              THEN public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
              ELSE public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
                   + c.topic_score
         END,
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.commenter_count,
         c.type_weight, COALESCE(c.topic_score, 0), now()
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
    topic_score         = EXCLUDED.topic_score,
    updated_at          = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;

  -- ---- entity_type_ranking reconcile, for these ids only (unchanged from 0098) ----
  DELETE FROM entity_type_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND NOT EXISTS (
      SELECT 1 FROM relations r
      WHERE r.from_entity_id = etr.entity_id
        AND r.type_id = types_prop
        AND r.to_entity_id = etr.type_id
    );

  INSERT INTO entity_type_ranking (type_id, entity_id, ranking_score)
  SELECT DISTINCT r.to_entity_id, r.from_entity_id, ers.ranking_score
  FROM relations r
  JOIN entity_ranking_scores ers ON ers.entity_id = r.from_entity_id
  WHERE r.from_entity_id = ANY(entity_ids)
    AND r.type_id = types_prop
  ON CONFLICT (type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score;

  -- ---- entity_topic_ranking reconcile, for these ids only (unchanged from 0098) ----
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

  -- The SCORE row count, as callers and the backfill script read it.
  RETURN affected;
END;
$$;
--> statement-breakpoint

-- Re-scores every topic debate, so a new Interested or debate on its topic reaches its
-- score. Hourly, from ranking-indexer's topic_ranking_reconcile run. Returns the debates scored.
CREATE OR REPLACE FUNCTION public.refresh_topic_debate_scores()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  ids uuid[];
BEGIN
  SELECT coalesce(array_agg(DISTINCT ty.from_entity_id), '{}') INTO ids
  FROM public.relations ty
  WHERE ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
    AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid
    AND EXISTS (SELECT 1 FROM public.relations tp
                WHERE tp.from_entity_id = ty.from_entity_id
                  AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)
    AND NOT EXISTS (SELECT 1 FROM public.relations c
                    WHERE c.from_entity_id = ty.from_entity_id
                      AND c.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid);
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_topic_debate_scores() IS E'@omit';
--> statement-breakpoint

-- The interest model's events: 0106_interested_vote_kind's body (gaia#1023, GEO-3158) copied
-- verbatim, plus one branch, topic debates. Votes, comments, claim debates, follows (Interested
-- only) and external signals are exactly as 0106 left them.
CREATE OR REPLACE FUNCTION personalization.user_interest_events(user_ids uuid[])
RETURNS TABLE (user_id uuid, kind text, source_id uuid, object_id uuid, object_is_topic boolean,
               occurred_at timestamptz)
LANGUAGE sql STABLE AS $$
  WITH RECURSIVE
  users AS (
    -- Only Personal spaces are people. A DAO space can author a comment or be named somewhere,
    -- and none of that is one person's interest.
    SELECT s.id FROM public.spaces s
    WHERE s.id = ANY(user_ids) AND s.type = 'Personal'
  ),
  comments AS (
    SELECT c.space_id AS user_id, c.from_entity_id AS comment_id, c.to_entity_id AS target
    FROM public.relations c
    JOIN users u ON u.id = c.space_id
    WHERE c.type_id = '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid
  ),
  -- A reply to a comment is about whatever the thread is about: walk up the Reply to chain until
  -- the target is not itself a comment. Bounded, in case the graph ever holds a cycle.
  thread(user_id, comment_id, target, depth) AS (
    SELECT user_id, comment_id, target, 0 FROM comments
    UNION ALL
    SELECT t.user_id, t.comment_id, up.to_entity_id, t.depth + 1
    FROM thread t
    CROSS JOIN LATERAL (
      SELECT r.to_entity_id FROM public.relations r
      WHERE r.from_entity_id = t.target AND r.type_id = '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid
      ORDER BY r.to_entity_id LIMIT 1
    ) up
    WHERE t.depth < 20
  ),
  comment_roots AS (
    SELECT DISTINCT ON (user_id, comment_id) user_id, comment_id, target
    FROM thread ORDER BY user_id, comment_id, depth DESC
  ),
  -- A user's held responses on entities: not removals (vote_type 2), people only.
  held AS (
    SELECT v.user_id, v.object_id, v.vote_kind, v.voted_at,
           -- Whether the object is typed Topic. Only needed for Interested, so only looked up there.
           v.vote_kind = 3 AND EXISTS (
             SELECT 1 FROM public.relations ty
             WHERE ty.from_entity_id = v.object_id
               AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
               AND ty.to_entity_id = '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid
           ) AS interested_in_topic
    FROM public.user_votes v
    WHERE v.user_id = ANY(user_ids) AND v.object_type = 0 AND v.vote_type IN (0, 1)
      AND EXISTS (SELECT 1 FROM users u WHERE u.id = v.user_id)
  ),
  -- Follows: a current Interested on a Topic (GEO-3158), and nothing else. Interested only ever
  -- holds vote_type 0, so `held` (which drops removals) is exactly the current Interesteds. One row
  -- per (user, topic), however many spaces the Interested was cast in.
  follows AS (
    SELECT h.user_id, h.object_id AS topic_id, max(h.voted_at) AS occurred_at
    FROM held h
    WHERE h.interested_in_topic
    GROUP BY h.user_id, h.object_id
  )
  -- Votes: any kind, either direction, on an entity -- except an Interested on a Topic, which is
  -- a follow (below), not a vote.
  SELECT h.user_id, 'vote'::text, h.object_id, h.object_id, false, max(h.voted_at)
  FROM held h
  WHERE NOT h.interested_in_topic
  GROUP BY h.user_id, h.object_id

  UNION ALL
  SELECT cr.user_id, 'comment', cr.target, cr.target, false,
         max(personalization.epoch_text_ts(e.created_at))
  FROM comment_roots cr
  JOIN public.entities e ON e.id = cr.comment_id
  GROUP BY cr.user_id, cr.target

  UNION ALL
  -- Debates: credited to each participant, for each thing debated.
  SELECT DISTINCT p.to_entity_id, 'debate', p.from_entity_id, dc.to_entity_id, false,
         personalization.epoch_text_ts(d.created_at)
  FROM public.relations p
  JOIN users u ON u.id = p.to_entity_id
  JOIN public.entities d ON d.id = p.from_entity_id
  JOIN public.relations dc ON dc.from_entity_id = p.from_entity_id
                          AND dc.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid
  WHERE p.type_id = '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid
    AND EXISTS (SELECT 1 FROM public.relations ty
                WHERE ty.from_entity_id = p.from_entity_id
                  AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                  AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)

  UNION ALL
  -- Topic debates (GEO-3088 "Topic debates", 0107): a Debate with no Claims relation of its own
  -- credits the topic it is filed under directly, at the same debate weight, to each participant.
  -- A claim debate never matches here (it has a Claims relation) and is credited above. Debates
  -- emit only 'debate', so joining one is never also a follow.
  SELECT DISTINCT p.to_entity_id, 'debate', p.from_entity_id, tp.to_entity_id, true,
         personalization.epoch_text_ts(d.created_at)
  FROM public.relations p
  JOIN users u ON u.id = p.to_entity_id
  JOIN public.entities d ON d.id = p.from_entity_id
  JOIN public.relations tp ON tp.from_entity_id = p.from_entity_id
                          AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
  WHERE p.type_id = '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid
    AND EXISTS (SELECT 1 FROM public.relations ty
                WHERE ty.from_entity_id = p.from_entity_id
                  AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                  AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)
    AND NOT EXISTS (SELECT 1 FROM public.relations c
                    WHERE c.from_entity_id = p.from_entity_id
                      AND c.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid)

  UNION ALL
  SELECT fo.user_id, 'follow', fo.topic_id, fo.topic_id, true, fo.occurred_at
  FROM follows fo

  UNION ALL
  SELECT x.user_id, x.kind, x.object_id, x.object_id, false, x.occurred_at
  FROM personalization.external_interest_signals x
  WHERE x.user_id = ANY(user_ids)
    AND EXISTS (SELECT 1 FROM users u WHERE u.id = x.user_id)
$$;
--> statement-breakpoint

UPDATE personalization.interest_signal_weights
   SET note = 'Took part in a published debate: a claim debate credits its claim''s topics, a topic debate its topic.'
 WHERE kind = 'debate';
--> statement-breakpoint

-- CREATE OR REPLACE keeps the function's grants; restated as 0100, 0105 and 0106 do.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
