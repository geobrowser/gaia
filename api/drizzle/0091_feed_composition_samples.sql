-- GEO-2926 follow-up: make the feed observable, so a ranking change cannot quietly
-- change what people see.
--
-- WHY THIS EXISTS. On 2026-09-16 two ranking retunes shipped in one night. #948 cut
-- participation_weight 7 -> 2.5 with a correct measurement, a passing test suite and a
-- reviewed migration, and it handed the default Explore feed to News stories:
--
--   top 22          Claim  Debate  News story
--   before #948        15       4           3
--   after  #948         5       1          16
--
-- Nothing caught it. Nothing could: ranking parameters have no "wrong" value that errors,
-- the suites assert orderings between fixtures rather than the shape of the real feed, and
-- the scores are stored, so the feed does not even change until a re-score runs. It was
-- caught by a person opening the app and saying "there are not enough claims", and the
-- before/after table above had to be RECONSTRUCTED after the fact from the arithmetic
-- relating the two parameter sets. That reconstruction only worked because the change was
-- confined to one term; a second change of any other kind and the prior state would have
-- been unrecoverable.
--
-- So: sample the composition of the ranked feed on a schedule and keep the history.
--
-- WHAT A SAMPLE IS. One row per run. `composition` is {type_id: count} over the top
-- `window_size` of the ranked walk restricted to `type_ids`, which is the window Explore
-- fetches before its diversity cap and per-space quota reorder it. Sampling the window
-- rather than the rendered page is deliberate: the cap can only interleave what the window
-- contains, so the window is where a composition problem is actually created, and it is
-- the layer this database can see.
--
-- THE CONFIG SNAPSHOT IS THE POINT, as much as the counts. Every sample records the
-- participation weight, cap and tau in force when it was taken. The values live in
-- production and were, until #948, recorded nowhere at all — weight 7 / cap 30 were set by
-- hand on 2026-08-20 and no artifact anywhere held them. A history of (parameters,
-- resulting feed shape) is what makes the next retune an argument from evidence instead of
-- a reconstruction under time pressure.
--
-- NOT per-space yet. `space_id` is here and nullable so per-space sampling can be added
-- without a migration; the sampler writes NULL, meaning "across all spaces", which is what
-- Explore's default feed ranks over. Yaniv's original complaint was about the top of a
-- single space, so per-space is the obvious next step — it is left out here only to keep
-- the first version's cost predictable.
CREATE TABLE IF NOT EXISTS "entity_feed_composition_samples" (
  "id"                   bigserial PRIMARY KEY,
  "sampled_at"           timestamptz NOT NULL DEFAULT now(),
  "space_id"             uuid,
  "window_size"          integer     NOT NULL,
  "type_ids"             uuid[]      NOT NULL,
  "composition"          jsonb       NOT NULL,
  "median_age_seconds"   bigint,
  "oldest_age_seconds"   bigint,
  "participation_weight" numeric     NOT NULL,
  "participation_cap"    numeric     NOT NULL,
  "tau_seconds"          numeric     NOT NULL
);
--> statement-breakpoint

-- Reads are "the recent history for this scope", always newest first.
CREATE INDEX IF NOT EXISTS "entity_feed_composition_samples_scope_idx"
  ON "entity_feed_composition_samples" ("space_id", "sampled_at" DESC);
--> statement-breakpoint

-- Takes the sample and returns its id.
--
-- The walk mirrors `entities_ranked_for_feed_by_type`: `entity_type_ranking` (0084) is the
-- denormalised table the typed feed reads, and ordering it by ranking_score DESC per type
-- and merging is what that function does. Reading the same table is the point — a sample
-- taken from a different source could disagree with the feed and nobody would know which
-- was right.
--
-- `p_window` defaults to 66, which is EXPLORE_DIVERSITY_WINDOW_SIZE in geogenesis: three
-- pages of 22. If that constant changes, this default is stale rather than wrong — the
-- caller passes the value explicitly and the sample records what it used.
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

  WITH candidates AS (
    -- One row per (type, entity). An entity carrying two watched types is counted under
    -- both, exactly as the typed feed would surface it under either.
    SELECT etr.type_id, etr.entity_id, s.ranking_score, e.created_at
      FROM public.entity_type_ranking etr
      JOIN public.entity_ranking_scores s ON s.entity_id = etr.entity_id
      JOIN public.entities e              ON e.id        = etr.entity_id
     WHERE etr.type_id = ANY(p_type_ids)
  ),
  window_rows AS (
    SELECT type_id, created_at
      FROM candidates
     ORDER BY ranking_score DESC
     LIMIT p_window
  ),
  counted AS (
    SELECT type_id, count(*) AS n FROM window_rows GROUP BY type_id
  ),
  ages AS (
    -- created_at is TEXT holding a unix epoch; anything unparseable is skipped rather
    -- than failing the sample, since one bad row must not cost the whole history.
    SELECT (EXTRACT(EPOCH FROM now())::bigint - created_at::bigint) AS age
      FROM window_rows
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
--> statement-breakpoint

-- The drift between the two most recent samples for a scope, one row per type that moved.
--
-- Returns nothing when there is only one sample, which is the correct answer to "what
-- changed" and not an error — the caller reports no drift rather than a first-run alarm.
--
-- Shares are of the window actually returned, not of `window_size`: a thin feed that
-- returns 40 rows of a 66-row window would otherwise read as every type collapsing at once.
CREATE OR REPLACE FUNCTION public.feed_composition_drift(p_space_id uuid DEFAULT NULL)
RETURNS TABLE (
  type_id        uuid,
  previous_count integer,
  current_count  integer,
  previous_share numeric,
  current_share  numeric,
  share_delta    numeric
)
LANGUAGE sql STABLE AS $$
  WITH recent AS (
    -- The outer ORDER BY is load-bearing: a window function is computed over the whole
    -- partition, so LIMIT without it would number the rows correctly and then keep an
    -- arbitrary two of them.
    --
    -- `id DESC` is not just a tiebreak. The whole suite is sent as one statement, so every
    -- sample taken in it shares a transaction timestamp and `sampled_at` cannot order them.
    SELECT id, composition, sampled_at,
           row_number() OVER (ORDER BY sampled_at DESC, id DESC) AS rn
      FROM public.entity_feed_composition_samples
     WHERE space_id IS NOT DISTINCT FROM p_space_id
     ORDER BY sampled_at DESC, id DESC
     LIMIT 2
  ),
  cur  AS (SELECT composition FROM recent WHERE rn = 1),
  prev AS (SELECT composition FROM recent WHERE rn = 2),
  cur_total  AS (SELECT GREATEST(COALESCE(sum(value::int), 0), 1) AS t
                   FROM cur, jsonb_each_text(cur.composition)),
  prev_total AS (SELECT GREATEST(COALESCE(sum(value::int), 0), 1) AS t
                   FROM prev, jsonb_each_text(prev.composition)),
  keys AS (
    SELECT key FROM cur, jsonb_each_text(cur.composition)
    UNION
    SELECT key FROM prev, jsonb_each_text(prev.composition)
  )
  SELECT
    k.key::uuid,
    COALESCE((SELECT value::int FROM prev, jsonb_each_text(prev.composition) WHERE key = k.key), 0),
    COALESCE((SELECT value::int FROM cur,  jsonb_each_text(cur.composition)  WHERE key = k.key), 0),
    round(COALESCE((SELECT value::numeric FROM prev, jsonb_each_text(prev.composition) WHERE key = k.key), 0)
          / (SELECT t FROM prev_total), 4),
    round(COALESCE((SELECT value::numeric FROM cur, jsonb_each_text(cur.composition) WHERE key = k.key), 0)
          / (SELECT t FROM cur_total), 4),
    round(COALESCE((SELECT value::numeric FROM cur, jsonb_each_text(cur.composition) WHERE key = k.key), 0)
          / (SELECT t FROM cur_total)
        - COALESCE((SELECT value::numeric FROM prev, jsonb_each_text(prev.composition) WHERE key = k.key), 0)
          / (SELECT t FROM prev_total), 4)
    FROM keys k
   WHERE EXISTS (SELECT 1 FROM prev)
   ORDER BY 6 DESC;
$$;
