-- GEO-2950: the canary was measuring the wrong feed.
--
-- 0091/0092 sample the ranked WINDOW — the rows `entities_ranked_for_feed_by_type` returns.
-- That is not what anyone sees. Explore then applies read-time reordering in geogenesis
-- (`applyDiversityCap`, and now `applyTargetMix`, followed by `applyPerSpaceQuota`), which
-- does not change what is in the window but does change which 22 of the 66 reach page one.
--
-- Measured on production 2026-09-17, featured spaces, the default three types:
--
--   per 10 items      window (what the canary recorded)   rendered page (what users saw)
--   Claim                        4.1                              1.8
--   Debate                       2.0                              5.9
--   News story                   3.9                              2.3
--
-- So the canary's "Claim 24 / Debate 8 / News 34" was true and useless: it would not have
-- moved when Preston's feed went claim-starved, because the window never changed. It took
-- three wrong answers to find that, and the fix is not to repoint it.
--
-- RECORD BOTH. The GAP between window and rendered is the diagnostic. A composition change
-- that shows in both is a ranking change; one that shows only in `rendered` is read-time
-- reordering; one that shows only in `window` is something the reader never saw. Tonight that
-- distinction was the whole answer, and it was not available.
--
-- `window` samples are computed in SQL by sample_feed_composition. `rendered` samples cannot
-- be — they require an HTTP call to the app's own feed endpoint — so the binary computes the
-- histogram and calls record_feed_composition_sample to store it. Storage stays in one place
-- and one shape either way.
ALTER TABLE "entity_feed_composition_samples"
  ADD COLUMN IF NOT EXISTS "source" text NOT NULL DEFAULT 'window';
--> statement-breakpoint

ALTER TABLE "entity_feed_composition_samples"
  DROP CONSTRAINT IF EXISTS "entity_feed_composition_samples_source_check";
--> statement-breakpoint

ALTER TABLE "entity_feed_composition_samples"
  ADD CONSTRAINT "entity_feed_composition_samples_source_check"
  CHECK ("source" IN ('window', 'rendered'));
--> statement-breakpoint

-- Reads are "the recent history for this scope AND source", always newest first.
DROP INDEX IF EXISTS "entity_feed_composition_samples_scope_idx";
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "entity_feed_composition_samples_scope_idx"
  ON "entity_feed_composition_samples" ("source", "space_id", "sampled_at" DESC);
--> statement-breakpoint

-- Store a composition the caller already computed. The rendered feed is only observable over
-- HTTP, so the measurement happens in the binary and this is the storage half.
--
-- Deliberately does NOT re-read entity_ranking_config from the caller's perspective: the
-- config snapshot is taken here, at write time, exactly as sample_feed_composition does, so
-- both sources record the parameters in force when the sample was taken.
CREATE OR REPLACE FUNCTION public.record_feed_composition_sample(
  p_source             text,
  p_type_ids           uuid[],
  p_composition        jsonb,
  p_window             integer,
  p_space_id           uuid    DEFAULT NULL,
  p_median_age_seconds bigint  DEFAULT NULL,
  p_oldest_age_seconds bigint  DEFAULT NULL
) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE
  v_id  bigint;
  v_cfg record;
BEGIN
  IF p_source IS NULL OR p_source NOT IN ('window', 'rendered') THEN
    RAISE EXCEPTION 'record_feed_composition_sample: source must be window or rendered, got %', p_source;
  END IF;
  IF p_type_ids IS NULL OR cardinality(p_type_ids) = 0 THEN
    RAISE EXCEPTION 'record_feed_composition_sample requires at least one type id';
  END IF;
  IF p_composition IS NULL OR jsonb_typeof(p_composition) <> 'object' THEN
    RAISE EXCEPTION 'record_feed_composition_sample requires a composition object';
  END IF;

  SELECT participation_weight, participation_cap, tau_seconds
    INTO v_cfg FROM public.entity_ranking_config WHERE id;

  INSERT INTO public.entity_feed_composition_samples (
    source, space_id, window_size, type_ids, composition,
    median_age_seconds, oldest_age_seconds,
    participation_weight, participation_cap, tau_seconds
  ) VALUES (
    p_source, p_space_id, p_window, p_type_ids, p_composition,
    p_median_age_seconds, p_oldest_age_seconds,
    v_cfg.participation_weight, v_cfg.participation_cap, v_cfg.tau_seconds
  ) RETURNING id INTO v_id;

  RETURN v_id;
END $$;
--> statement-breakpoint

-- Adding a parameter creates a SIBLING overload rather than replacing the function, and then
-- every existing one-argument call fails with
--
--   ERROR: function public.feed_composition_drift(unknown) is not unique
--
-- which is every call the binary makes. Drop the old signature first. 0077 and 0085 hit this
-- and handled it the same way; drizzle runs a migration in one transaction, so no concurrent
-- caller observes the window where the function does not exist.
DROP FUNCTION IF EXISTS public.feed_composition_drift(uuid);
--> statement-breakpoint

-- Drift is now per (source, scope). Comparing a rendered sample against a window one would
-- report a change every single run, since they legitimately differ.
CREATE OR REPLACE FUNCTION public.feed_composition_drift(
  p_space_id uuid DEFAULT NULL,
  p_source   text DEFAULT 'window'
)
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
    -- arbitrary two of them. `id DESC` is not just a tiebreak — samples taken in one
    -- statement share a transaction timestamp and `sampled_at` cannot order them.
    SELECT id, composition, sampled_at,
           row_number() OVER (ORDER BY sampled_at DESC, id DESC) AS rn
      FROM public.entity_feed_composition_samples
     WHERE space_id IS NOT DISTINCT FROM p_space_id
       AND source = p_source
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
