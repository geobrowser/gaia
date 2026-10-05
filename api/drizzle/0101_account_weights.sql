-- GEO-3141: how much each account's votes count towards OTHER people's personalization.
--
-- Once votes shape what other people see (shared claim scores, anchor claims, matchmaking, the
-- stance map), throwaway and test voters are worth someone's time, and test accounts already
-- distort the data. Every voting account gets a weight in [0, 1] and the reasons behind it.
--
-- The weight applies only to aggregation over other users. An account's OWN For you still
-- personalizes from its own activity whatever its weight — callers aggregating across users
-- multiply by `account_vote_weight(user_id)`; per-user reads ignore it.
--
-- Both tables and both functions are private: @omit keeps them out of the public GraphQL API,
-- and api/src/kg/securitySignals.ts lists them in HIDDEN_SURFACE so a request naming them
-- trips the probe alarm.

-- Accounts that must carry weight 0, with why. Test accounts are known only to analytics
-- (labels, emails, Privy ids), and gaia knows only personal spaces, so the list is synced in
-- nightly by `replace_account_exclusions` rather than computed here. Analytics lag or an
-- outage then only delays new exclusions; it can never stop scoring.
CREATE TABLE IF NOT EXISTS "account_exclusions" (
	"user_id" uuid PRIMARY KEY NOT NULL,
	"reason" text NOT NULL,
	"source" text DEFAULT 'analytics' NOT NULL,
	"synced_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
COMMENT ON TABLE "account_exclusions" IS E'@omit';
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "account_weights" (
	"user_id" uuid PRIMARY KEY NOT NULL,
	"weight" double precision NOT NULL,
	-- [{"code": "...", "detail": "..."}], one entry per rule that moved the weight.
	"reasons" jsonb NOT NULL,
	"vote_count" integer NOT NULL,
	"stance_vote_count" integer NOT NULL,
	"first_vote_at" timestamp with time zone,
	"computed_at" timestamp with time zone NOT NULL,
	CONSTRAINT "account_weights_weight_range" CHECK ("weight" >= 0 AND "weight" <= 1)
);
--> statement-breakpoint
COMMENT ON TABLE "account_weights" IS E'@omit';
--> statement-breakpoint

-- Recomputes every voter's weight. Run hourly by the ranking-indexer `account-weights`
-- CronJob (the ticket asks for at least nightly). One statement per table, in one
-- transaction, so readers see either the old weights or the new ones.
--
-- Rules, each recorded in `reasons` when it applies:
--   * excluded: weight 0, reason copied from account_exclusions.
--   * new_account: weight ramps linearly from 0 to 1 over `ramp_days` after the first vote.
--   * one_sided: with at least `one_sided_min_votes` stance votes (Agree/Disagree), of which
--     more than `one_sided_share` go one way, the weight is multiplied by `one_sided_factor`.
-- The thresholds are starting values from the spec and are parameters, so tuning them needs
-- no migration.
CREATE OR REPLACE FUNCTION public.refresh_account_weights(
	ramp_days double precision DEFAULT 7,
	one_sided_min_votes integer DEFAULT 20,
	one_sided_share double precision DEFAULT 0.95,
	one_sided_factor double precision DEFAULT 0.25,
	as_of timestamp with time zone DEFAULT now()
)
RETURNS integer
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
	written integer;
BEGIN
	IF ramp_days <= 0 OR one_sided_min_votes < 1 OR one_sided_share <= 0.5 OR one_sided_share > 1
	   OR one_sided_factor < 0 OR one_sided_factor > 1 THEN
		RAISE EXCEPTION 'refresh_account_weights: invalid parameters' USING ERRCODE = '22023';
	END IF;

	WITH voters AS (
		SELECT
			uv.user_id,
			count(*)::integer AS vote_count,
			count(*) FILTER (WHERE uv.vote_kind = 1)::integer AS stance_votes,
			count(*) FILTER (WHERE uv.vote_kind = 1 AND uv.vote_type = 0)::integer AS agrees,
			min(uv.voted_at) AS first_vote_at
		FROM public.user_votes uv
		GROUP BY uv.user_id
	),
	scored AS (
		SELECT
			v.*,
			x.reason AS excluded_reason,
			least(1.0, greatest(0.0, extract(epoch FROM (as_of - v.first_vote_at)) / (ramp_days * 86400.0))) AS ramp,
			CASE
				WHEN v.stance_votes >= one_sided_min_votes
				 AND greatest(v.agrees, v.stance_votes - v.agrees)::double precision / v.stance_votes > one_sided_share
				THEN true ELSE false
			END AS one_sided
		FROM voters v
		LEFT JOIN public.account_exclusions x ON x.user_id = v.user_id
	),
	weighted AS (
		SELECT
			s.*,
			CASE
				WHEN s.excluded_reason IS NOT NULL THEN 0.0
				ELSE s.ramp * CASE WHEN s.one_sided THEN one_sided_factor ELSE 1.0 END
			END AS weight,
			(jsonb_build_array(
				CASE WHEN s.excluded_reason IS NOT NULL
					THEN jsonb_build_object('code', 'excluded', 'detail', s.excluded_reason) END,
				CASE WHEN s.excluded_reason IS NULL AND s.ramp < 1
					THEN jsonb_build_object('code', 'new_account',
						'detail', format('first vote %s; full weight after %s days', s.first_vote_at, ramp_days)) END,
				CASE WHEN s.excluded_reason IS NULL AND s.one_sided
					THEN jsonb_build_object('code', 'one_sided',
						'detail', format('%s of %s stance votes one way', greatest(s.agrees, s.stance_votes - s.agrees), s.stance_votes)) END
			)) AS raw_reasons
		FROM scored s
	)
	INSERT INTO public.account_weights AS aw
		(user_id, weight, reasons, vote_count, stance_vote_count, first_vote_at, computed_at)
	SELECT
		w.user_id,
		w.weight,
		-- jsonb_build_array keeps the NULL slots of rules that did not apply; drop them.
		coalesce((SELECT jsonb_agg(r) FROM jsonb_array_elements(w.raw_reasons) r WHERE r <> 'null'::jsonb), '[]'::jsonb),
		w.vote_count,
		w.stance_votes,
		w.first_vote_at,
		as_of
	FROM weighted w
	ON CONFLICT (user_id) DO UPDATE SET
		weight = excluded.weight,
		reasons = excluded.reasons,
		vote_count = excluded.vote_count,
		stance_vote_count = excluded.stance_vote_count,
		first_vote_at = excluded.first_vote_at,
		computed_at = excluded.computed_at;
	GET DIAGNOSTICS written = ROW_COUNT;

	-- An account whose votes are all gone has no weight to give.
	DELETE FROM public.account_weights aw
	WHERE NOT EXISTS (SELECT 1 FROM public.user_votes uv WHERE uv.user_id = aw.user_id);

	RETURN written;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_account_weights(double precision, integer, double precision, double precision, timestamp with time zone) IS E'@omit';
--> statement-breakpoint

-- Atomically replaces the analytics-sourced exclusions with `rows`, a JSON array of
-- {"user_id": "<uuid>", "reason": "..."}. Rows from other sources (a manual exclusion, say)
-- are left alone. Called by the nightly sync; an empty array is refused, because it would
-- otherwise re-weight every test account to full on a bad analytics read.
CREATE OR REPLACE FUNCTION public.replace_account_exclusions(rows jsonb, source_name text DEFAULT 'analytics')
RETURNS integer
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
	written integer;
BEGIN
	IF rows IS NULL OR jsonb_typeof(rows) <> 'array' OR jsonb_array_length(rows) = 0 THEN
		RAISE EXCEPTION 'replace_account_exclusions: refusing an empty or non-array list' USING ERRCODE = '22023';
	END IF;

	DELETE FROM public.account_exclusions WHERE source = source_name;
	INSERT INTO public.account_exclusions (user_id, reason, source, synced_at)
	SELECT DISTINCT ON ((r ->> 'user_id')::uuid) (r ->> 'user_id')::uuid, r ->> 'reason', source_name, now()
	FROM jsonb_array_elements(rows) r
	ON CONFLICT (user_id) DO NOTHING;
	GET DIAGNOSTICS written = ROW_COUNT;
	RETURN written;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.replace_account_exclusions(jsonb, text) IS E'@omit';
--> statement-breakpoint

-- The weight to apply when aggregating `user_id`'s votes into other people's personalization.
-- An account not yet scored counts 0: it is at most an hour old as a voter, and the new-account
-- ramp would give it almost nothing anyway.
CREATE OR REPLACE FUNCTION public.account_vote_weight(voter uuid)
RETURNS double precision
LANGUAGE sql STABLE PARALLEL SAFE AS $$
	SELECT coalesce((SELECT weight FROM public.account_weights WHERE user_id = voter), 0.0)
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.account_vote_weight(uuid) IS E'@omit';
