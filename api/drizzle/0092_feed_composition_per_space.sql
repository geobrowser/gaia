-- GEO-2926 follow-up: sample the feed the way the feed is actually built, and per space.
--
-- Two problems with 0091, both found by reading it back rather than by it failing.
--
-- 1. IT HAND-ROLLED THE WALK. 0091 ordered `entity_type_ranking` itself instead of calling
--    `entities_ranked_for_feed_by_type`, the function Explore actually queries. So it
--    applied NONE of the feed's predicates: an entity with no name, a system entity, a
--    blocklisted entity or an editorially excluded type could occupy a slot in the sample
--    that it could never occupy in the feed. A canary that measures something adjacent to
--    the feed reports drift the feed does not have, and misses drift it does.
--
-- 2. IT COUNTED (type, entity) PAIRS. An entity carrying two watched types took two of the
--    66 slots, where the real feed shows it once — `entities_ranked_for_feed_by_type` does
--    `DISTINCT ON (entity_id)`. The window therefore held fewer distinct entities than the
--    feed's, and the histogram summed to more than the feed would show.
--
-- Neither was visible in the first production sample (24 + 8 + 34 = 66 exactly), which is
-- the point: a sampler that is subtly wrong looks exactly like one that is right until the
-- day it is load-bearing.
--
-- PER SPACE. `space_id` was already on the table and already a parameter, but 0091 only
-- RECORDED it — the walk ignored it entirely, so every sample was global whatever was
-- passed. That was the thing worth fixing: Yaniv's original complaint was about the top of
-- a single space, and a global canary would not have caught it.
--
-- `max_per_type` is passed as the window size. That is correctness, not tuning: the
-- function caps each type's candidate list BEFORE the global ordering, so anything below
-- `offset + first` silently returns short. At offset 0 that is the window itself.
CREATE OR REPLACE FUNCTION public.sample_feed_composition(
  p_type_ids uuid[],
  p_window   integer DEFAULT 66,
  p_space_id uuid    DEFAULT NULL
) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE
  v_id bigint;
  v_cfg record;
BEGIN
  IF p_type_ids IS NULL OR cardinality(p_type_ids) = 0 THEN
    RAISE EXCEPTION 'sample_feed_composition requires at least one type id';
  END IF;
  IF p_window IS NULL OR p_window <= 0 THEN
    RAISE EXCEPTION 'sample_feed_composition requires a positive window, got %', p_window;
  END IF;

  SELECT participation_weight, participation_cap, tau_seconds
    INTO v_cfg FROM public.entity_ranking_config WHERE id;

  WITH win AS (
    -- The same call Explore makes. NULL space_ids means "across all spaces", which is what
    -- the function already treats as unscoped.
    SELECT f.id, f.created_at
      FROM public.entities_ranked_for_feed_by_type(
             p_type_ids,
             NULL, NULL, NULL,
             CASE WHEN p_space_id IS NULL THEN NULL ELSE ARRAY[p_space_id] END,
             p_window
           ) f
     LIMIT p_window
  ),
  labelled AS (
    -- One type per entity, so the histogram sums to the number of rows in the window.
    -- Earliest match in the caller's own type order, which is deterministic and stable;
    -- the feed's card shows types from its display space, which this database cannot see,
    -- so any choice here is a convention rather than a reproduction of the UI.
    SELECT w.id, w.created_at,
           (SELECT etr.type_id
              FROM public.entity_type_ranking etr
             WHERE etr.entity_id = w.id
               AND etr.type_id = ANY(p_type_ids)
             ORDER BY array_position(p_type_ids, etr.type_id)
             LIMIT 1) AS type_id
      FROM win w
  ),
  counted AS (
    SELECT type_id, count(*) AS n FROM labelled WHERE type_id IS NOT NULL GROUP BY type_id
  ),
  ages AS (
    -- created_at is TEXT holding a unix epoch; anything unparseable is skipped rather than
    -- failing the sample, since one bad row must not cost the whole history.
    SELECT (EXTRACT(EPOCH FROM now())::bigint - created_at::bigint) AS age
      FROM labelled
     WHERE created_at ~ '^[0-9]+$'
  )
  INSERT INTO public.entity_feed_composition_samples (
    space_id, window_size, type_ids, composition,
    median_age_seconds, oldest_age_seconds,
    participation_weight, participation_cap, tau_seconds
  )
  SELECT
    p_space_id,
    p_window,
    p_type_ids,
    COALESCE((SELECT jsonb_object_agg(type_id::text, n) FROM counted), '{}'::jsonb),
    (SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY age)::bigint FROM ages),
    (SELECT max(age) FROM ages),
    v_cfg.participation_weight, v_cfg.participation_cap, v_cfg.tau_seconds
  RETURNING id INTO v_id;

  RETURN v_id;
END $$;
