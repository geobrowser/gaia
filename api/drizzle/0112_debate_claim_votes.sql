-- GEO-3461 phase 1b: claim votes cast in a debate's own UI count toward the debate in Best.
--
-- WHY. GEO-3454 replaces a debate's up/down with Agree/Disagree on its claim, and people already
-- vote on claims from the debate player: the claims panel, the claim ticker, the end card. Those
-- votes land on the claim, so the debate they were cast in never sees them. analytics 116 now
-- counts them per debate and hour in `feed_signal_items.debate_claim_votes`: succeeded up/down
-- votes on a claim whose event carries the debate's `debate_id`, from any page. Withdrawals, the
-- winner vote and live-room votes are not counted (see that migration).
--
-- THE TERM. One more addend in 0111's watch term, inside its clamp:
--
--   + claim_votes_weight × ln(1 + claim votes over the last watch_window_days)
--
-- 0.5 by default: between plays (0.25) and completed plays (1.0), the order the ticket gives.
-- Measured on testnet 2026-10-23 over 30 days: 252 such votes on 64 debates, about 4 each, which is
-- worth 0.8 here; 20 votes are worth 1.5, about 1.7 days of recency.
--
-- No other score changes: refresh_entity_ranking_scores reads only debate_watch_scores' entity_id
-- and watch_score, and refresh_debate_watch_scores already re-scores every debate with signal rows,
-- which now include these.

ALTER TABLE "personalization"."feed_item_signals_hourly"
  ADD COLUMN IF NOT EXISTS "debate_claim_votes" integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_claim_votes_weight" numeric NOT NULL DEFAULT 0.5;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_watch_claim_votes_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_watch_claim_votes_valid"
      CHECK ("watch_claim_votes_weight" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- 0109's replace, with the new column. Everything else is unchanged.
CREATE OR REPLACE FUNCTION personalization.replace_feed_signals(
	p_generation text,
	p_window_start timestamptz,
	p_window_end timestamptz,
	p_items jsonb,
	p_users jsonb,
	p_allow_empty boolean DEFAULT false,
	p_as_of timestamptz DEFAULT now()
)
RETURNS TABLE (run_id bigint, item_rows integer, user_rows_in integer, user_rows_placed integer,
               user_rows integer, item_rows_expired integer, user_rows_expired integer)
LANGUAGE plpgsql AS $$
DECLARE
	cfg personalization.feed_signal_config%ROWTYPE;
	start_day date := (p_window_start AT TIME ZONE 'UTC')::date;
	n_items integer;
	n_users_in integer;
	n_placed integer;
	n_users integer;
	n_items_expired integer;
	n_users_expired integer;
	n_existing integer;
	new_id bigint;
BEGIN
	IF p_window_start IS NULL OR p_window_end IS NULL OR p_window_end <= p_window_start THEN
		RAISE EXCEPTION 'replace_feed_signals: empty window [%, %)', p_window_start, p_window_end;
	END IF;
	IF p_window_start <> date_trunc('day', p_window_start, 'UTC') THEN
		RAISE EXCEPTION 'replace_feed_signals: window_start % is not a UTC midnight', p_window_start;
	END IF;
	IF jsonb_typeof(p_items) IS DISTINCT FROM 'array' OR jsonb_typeof(p_users) IS DISTINCT FROM 'array' THEN
		RAISE EXCEPTION 'replace_feed_signals: p_items and p_users must be JSON arrays';
	END IF;
	SELECT * INTO cfg FROM personalization.feed_signal_config WHERE id;

	-- One writer at a time; the CronJob forbids overlap too.
	PERFORM pg_advisory_xact_lock(hashtext('personalization.feed_signals'));

	IF jsonb_array_length(p_items) = 0 AND NOT p_allow_empty THEN
		SELECT count(*) INTO n_existing FROM personalization.feed_item_signals_hourly h WHERE h.hour >= p_window_start;
		IF n_existing > 0 THEN
			RAISE EXCEPTION 'replace_feed_signals: analytics returned no items for a window holding % rows; refusing to erase them', n_existing;
		END IF;
	END IF;

	DELETE FROM personalization.feed_item_signals_hourly h WHERE h.hour >= p_window_start;
	-- A row from an analytics view without debate_claim_votes (before 116) stores 0.
	INSERT INTO personalization.feed_item_signals_hourly
		(item_id, hour, impressions, impressions_top3, impressions_4_10, impressions_11_30, impressions_31_plus,
		 opens, votes, plays, explore_plays, watch_ms, explore_watch_ms, plays_with_duration, watch_fraction_sum,
		 completed_plays, debate_claim_votes)
	SELECT r.item_id, r.hour, r.impressions, r.impressions_top3, r.impressions_4_10, r.impressions_11_30,
	       r.impressions_31_plus, r.opens, r.votes, r.plays, r.explore_plays, r.watch_ms, r.explore_watch_ms,
	       r.plays_with_duration, r.watch_fraction_sum, r.completed_plays, COALESCE(r.debate_claim_votes, 0)
	FROM jsonb_to_recordset(p_items) AS r(
		item_id uuid, hour timestamptz, impressions integer, impressions_top3 integer, impressions_4_10 integer,
		impressions_11_30 integer, impressions_31_plus integer, opens integer, votes integer, plays integer,
		explore_plays integer, watch_ms bigint, explore_watch_ms bigint, plays_with_duration integer,
		watch_fraction_sum double precision, completed_plays integer, debate_claim_votes integer)
	WHERE r.item_id IS NOT NULL AND r.hour >= p_window_start AND r.hour < p_window_end;
	GET DIAGNOSTICS n_items = ROW_COUNT;

	-- Place each wallet hash on a personal space. Several rows can land on one (user, day, item)
	-- only if one account holds several wallets that are all personal-space addresses; sum them.
	CREATE TEMP TABLE IF NOT EXISTS incoming_feed_seen (
		user_id uuid, day date, item_id uuid, impressions integer, last_seen_at timestamptz
	) ON COMMIT DROP;
	TRUNCATE incoming_feed_seen;
	n_users_in := jsonb_array_length(p_users);
	INSERT INTO incoming_feed_seen
	SELECT s.id, r.day, r.item_id, r.impressions, r.last_seen_at
	FROM jsonb_to_recordset(p_users) AS r(wallet_address_hash text, item_id uuid, day date, impressions integer, last_seen_at timestamptz)
	JOIN public.spaces s
	  ON s.type = 'Personal'
	 AND personalization.wallet_address_hash(s.address) = lower(btrim(r.wallet_address_hash))
	WHERE r.item_id IS NOT NULL AND r.day >= start_day AND r.last_seen_at < p_window_end;
	GET DIAGNOSTICS n_placed = ROW_COUNT;

	IF n_users_in > 0 AND n_placed = 0 AND NOT p_allow_empty THEN
		SELECT count(*) INTO n_existing FROM personalization.feed_user_seen_daily u WHERE u.day >= start_day;
		IF n_existing > 0 THEN
			RAISE EXCEPTION 'replace_feed_signals: none of % user rows is on a personal space while the window holds %; the identity mapping looks broken',
				n_users_in, n_existing;
		END IF;
	END IF;

	DELETE FROM personalization.feed_user_seen_daily u WHERE u.day >= start_day;
	INSERT INTO personalization.feed_user_seen_daily (user_id, day, item_id, impressions, last_seen_at)
	SELECT i.user_id, i.day, i.item_id, sum(i.impressions), max(i.last_seen_at)
	FROM incoming_feed_seen i
	GROUP BY i.user_id, i.day, i.item_id;
	GET DIAGNOSTICS n_users = ROW_COUNT;

	-- Retention, every run.
	DELETE FROM personalization.feed_user_seen_daily u
	WHERE u.day < (p_as_of AT TIME ZONE 'UTC')::date - cfg.user_retention_days;
	GET DIAGNOSTICS n_users_expired = ROW_COUNT;
	DELETE FROM personalization.feed_item_signals_hourly h
	WHERE h.hour < p_as_of - make_interval(days => cfg.item_retention_days);
	GET DIAGNOSTICS n_items_expired = ROW_COUNT;
	DELETE FROM personalization.feed_signal_runs r
	WHERE r.finished_at < p_as_of - make_interval(days => cfg.run_retention_days);

	INSERT INTO personalization.feed_signal_runs
		(finished_at, status, generation, data_through, window_start, window_end, item_rows, user_rows_in,
		 user_rows_placed, user_rows, item_rows_expired, user_rows_expired)
	VALUES (p_as_of, 'succeeded', p_generation, p_window_end, p_window_start, p_window_end, n_items, n_users_in,
	        n_placed, n_users, n_items_expired, n_users_expired)
	RETURNING id INTO new_id;

	RETURN QUERY SELECT new_id, n_items, n_users_in, n_placed, n_users, n_items_expired, n_users_expired;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.replace_feed_signals(text, timestamptz, timestamptz, jsonb, jsonb, boolean, timestamptz) IS E'@omit';
--> statement-breakpoint

-- 0111's term plus the claim-vote addend, and the count among its inputs. The result type changes,
-- so drop first; refresh_entity_ranking_scores (plpgsql) resolves it at call time.
DROP FUNCTION IF EXISTS public.debate_watch_scores(uuid[]);
--> statement-breakpoint
CREATE FUNCTION public.debate_watch_scores(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, impressions bigint, plays bigint, explore_watch_seconds double precision,
               completed_plays bigint, claim_votes bigint, watch_score numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.* FROM public.entity_ranking_config c WHERE c.id
  ),
  debates AS (
    SELECT DISTINCT ty.from_entity_id AS id
    FROM public.relations ty
    WHERE ty.from_entity_id = ANY(entity_ids)
      AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
      AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid -- Debate
  ),
  signals AS (
    SELECT f.item_id AS id,
           sum(f.impressions)::bigint AS impressions,
           sum(f.plays)::bigint AS plays,
           sum(f.explore_watch_ms)::double precision / 1000 AS explore_watch_seconds,
           sum(f.completed_plays)::bigint AS completed_plays,
           sum(f.debate_claim_votes)::bigint AS claim_votes
    FROM personalization.feed_item_signals_hourly f CROSS JOIN cfg
    WHERE f.item_id IN (SELECT id FROM debates)
      AND f.hour >= now() - make_interval(days => cfg.watch_window_days)
    GROUP BY f.item_id
  ),
  inputs AS (
    SELECT d.id,
           COALESCE(s.impressions, 0) AS impressions,
           COALESCE(s.plays, 0) AS plays,
           COALESCE(s.explore_watch_seconds, 0) AS explore_watch_seconds,
           COALESCE(s.completed_plays, 0) AS completed_plays,
           COALESCE(s.claim_votes, 0) AS claim_votes
    FROM debates d LEFT JOIN signals s ON s.id = d.id
  )
  SELECT i.id, i.impressions, i.plays, i.explore_watch_seconds, i.completed_plays, i.claim_votes,
         GREATEST(-cfg.watch_cap, LEAST(cfg.watch_cap,
             cfg.watch_ratio_weight
               * ln(((GREATEST(i.explore_watch_seconds, 0)::numeric
                      + cfg.watch_prior_impressions * cfg.watch_reference_seconds)
                     / (GREATEST(i.impressions, 0) + cfg.watch_prior_impressions))
                    / cfg.watch_reference_seconds)
           + cfg.watch_completed_weight * ln(1 + GREATEST(i.completed_plays, 0)::numeric)
           + cfg.watch_plays_weight * ln(1 + GREATEST(i.plays, 0)::numeric)
           + cfg.watch_claim_votes_weight * ln(1 + GREATEST(i.claim_votes, 0)::numeric)))
  FROM inputs i CROSS JOIN cfg
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.debate_watch_scores(uuid[]) IS E'@omit';
--> statement-breakpoint

-- No rows carry claim votes yet (the next feed_signals_sync writes them), but re-score anyway so
-- the new function is exercised once at deploy.
SELECT public.refresh_debate_watch_scores();
