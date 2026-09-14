-- Explore feed "Best": let curation votes count as participation, so entities whose
-- only engagement axis is curation can compete at all.
--
-- THE DEFECT (GEO-2853 / GEO-2690)
--
-- Reported 2026-09-14: the Explore feed shows nothing but Claims. Measured against
-- live data, every one of the 66 rows Explore fetches is a Claim, the first Debate
-- sits at rank 198, and 50 Debates exist in those spaces. The diversity cap cannot
-- help — it reorders the window it is given, and that window holds no Debates.
--
-- The cause is `participation_score`, which 0078 added deliberately as an UNBOUNDED
-- log term so engagement could outrun recency, and which reads `vote_kind = 1`
-- (stance) only:
--
--   claims          participation up to +20.97   (stance: AGREED / DISAGREED)
--   all 68 debates  participation exactly  0.00
--
-- Debates are not unengaged. They carry 297 votes — but as `vote_kind = 0`
-- (curation), which feeds only the Wilson quality term. That term is BOUNDED:
-- `ln(wilson)` spans [-4.6, 0]. So claim engagement lands in an unbounded term and
-- debate engagement lands in a ~5-unit one, and no amount of debate engagement can
-- close the gap. Debates already score better on quality (0.49 vs 0.09) and still
-- lose by 15+ points.
--
-- WHY NOT THE TYPE WEIGHT
--
-- Because it cannot work, which 0073's `entity_type_weights` ceiling makes exact.
-- Debate is already weighted 1.5 against Claim's 1.0, and the CHECK constraint caps
-- weight at 10.0. Simulated at that ceiling on live data the best Debate reaches
-- rank 115 — still outside the 66-row window, still zero Debates in it. The most the
-- lever can buy is `ln(10) - ln(1.5)` = 1.897 against a 15.2-point gap. This is what
-- GEO-2690 concluded in its title.
--
-- THE CHANGE
--
-- One expression. `participation` becomes stance + curation rather than stance alone.
-- The `votes` CTE that supplies curation already exists — it feeds `quality_score` —
-- so nothing new is read.
--
-- The two terms measure different things and both should count: Wilson is a RATIO
-- (how well received, bounded by construction), participation is VOLUME (how many
-- people engaged, unbounded on purpose). Counting a curation vote in both is the
-- same shape as 0078's own stated reference, Reddit's `log10(score) + t/45000`,
-- where the score term is unbounded volume. It is not double-counting a single
-- signal; it is the ratio and the volume of that signal, which is the asymmetry the
-- formula family assumes.
--
-- Veracity (`vote_kind = 2`) is deliberately NOT included. It is an assessment of
-- truth rather than engagement, there is no veracity CTE to reuse, and at 104 votes
-- across the whole table it would change nothing measurable. Adding it later is a
-- one-line change of the same shape.
--
-- MEASURED, on live data, with the armed tunables (weight 7, cap 30)
--
--   debates in Explore's 66-row window   0  ->  11
--   debates on page one (22 rows)        0  ->   5
--   rows changed of the 66              24        (42 hold, so not a reshuffle)
--   best debate's rank                 198  ->   #5
--
-- REQUIRES A RE-SCORE. `refresh_entity_ranking_scores` only rewrites rows it is
-- called for, so this changes nothing until affected entities are re-scored. That is
-- 3,353 entities — those carrying curation votes — and it ships as a separate script
-- rather than inline here, for 0073's reason: `api`'s initContainer runs
-- `db:migrate`, so a long statement in a migration stalls every deploy.

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
           -- Curation (`v`) joins stance (`s`) here; see the header for why.
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
  RETURN affected;
END;
$$;
