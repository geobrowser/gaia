-- GEO-3146: the stance map, in SHADOW.
--
-- The stance map places every voter on a few shared axes, so that two users who never voted on the
-- same claim can still be compared. It is gated on data (the spec's gate: about 10k clean votes,
-- and claim positions on the main axis that correlate above 0.7 between two random halves of the
-- voters). On 2 Oct there were 4,474 votes and the half-split correlation was 0.1-0.2.
--
-- SHADOW means: ranking-indexer's `stance_map_shadow` CronJob fits and evaluates the map once a
-- day and records how good it is, so we know the day it clears the gate. NOTHING reads the map:
-- no feed, no matchmaking, no API. This migration stores the evaluation only.
--
-- NO POSITIONS ARE STORED. Per-user positions are an inferred political stance, which may be a
-- "political opinion" under GDPR Art. 9. While nothing uses them there is no reason to keep them,
-- so the job fits them in memory, evaluates, and drops them. Storing them is a decision for the
-- day the map leaves shadow, together with a consent/DPIA decision.
--
-- INPUTS, read by the job through the two functions below:
--   * votes: each user's latest Agree/Disagree (vote_kind 1, vote_type 0/1) on a Claim-typed
--     entity, in any space, as in 0103. Weighted by account_vote_weight (0101), and accounts
--     weighing 0 (test, excluded) are left out entirely.
--   * stated: a debater's side in a claim debate. A Debate entity (fd51f935) names its main claim
--     with Claims (e614cce1) and its sides with Supported by (d19fad56) and Opposed by (c57de77c),
--     each to the debater's personal space id. Supported by is a stated Agree with the main claim,
--     Opposed by a stated Disagree. The job counts these towards the debater's POSITION but not
--     their overall agree TENDENCY (GEO-3146 acceptance criterion): a debater who argued For did
--     not thereby become someone who agrees with things. Topic debates have no Claims relation, so
--     they state nothing, and a debate naming more than one main claim is skipped as ambiguous.
--   * seeds: Supports (81faa4ad) and Opposes (71d1bcd5) from an extracted claim to its debate's
--     main claim (GEO-3142). The job uses them as priors: a supporting claim is pulled towards the
--     main claim's position, an opposing one towards its mirror image. Addresses (7115a43d) carries
--     no direction and is not a seed.
--
-- PRIVACY. Everything here is in the `personalization` schema, which PostGraphile does not
-- introspect, and the names are listed in api/src/kg/securitySignals.ts HIDDEN_SURFACE. The run
-- table holds aggregate counts and scores only, never a user id.

CREATE SCHEMA IF NOT EXISTS "personalization";
--> statement-breakpoint

-- The gate, and the job's knobs. One row. Changing a threshold here re-gates from the next run.
CREATE TABLE IF NOT EXISTS "personalization"."stance_map_config" (
	"id" boolean PRIMARY KEY DEFAULT true NOT NULL,
	-- The spec's gate.
	"min_clean_votes" integer DEFAULT 10000 NOT NULL,
	"min_split_correlation" double precision DEFAULT 0.7 NOT NULL,
	-- Not in the spec, but a map that predicts held-out votes no better than each user's and
	-- claim's tendencies has learned nothing worth matching on, however stable it is.
	"min_auc_lift" double precision DEFAULT 0.0 NOT NULL,
	-- How much a debater's stated side counts relative to one vote.
	"stated_weight" double precision DEFAULT 1.0 NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "stance_map_config_one_row" CHECK ("id"),
	CONSTRAINT "stance_map_config_ranges" CHECK (
		"min_clean_votes" >= 0 AND "min_split_correlation" BETWEEN 0 AND 1
		AND "stated_weight" >= 0)
);
--> statement-breakpoint
INSERT INTO "personalization"."stance_map_config" ("id") VALUES (true) ON CONFLICT DO NOTHING;
--> statement-breakpoint

-- One row per run: how good the map is today, and whether it clears the gate. Kept as history
-- (it is a few hundred bytes a day), so the trend towards the gate is visible.
CREATE TABLE IF NOT EXISTS "personalization"."stance_map_runs" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"ran_at" timestamp with time zone DEFAULT now() NOT NULL,
	"model_version" text NOT NULL,
	-- Data volume. clean_votes counts stance votes on claims by accounts weighing more than 0;
	-- weighted_votes is the same votes summed by account weight.
	"clean_votes" integer NOT NULL,
	"weighted_votes" double precision NOT NULL,
	"voters" integer NOT NULL,
	"claims" integer NOT NULL,
	"stated_positions" integer NOT NULL,
	"seeded_claims" integer NOT NULL,
	-- The chosen model: k axes and its regularisation, picked on a validation split.
	"axes" integer,
	"lambda" double precision,
	-- Held-out vote prediction (ROC AUC), mean over the evaluation repeats.
	"auc_map" double precision,
	"auc_tendencies" double precision,
	"auc_lift" double precision,
	-- The same map fitted without the claim-stance seeds, to show whether they help.
	"auc_map_unseeded" double precision,
	"test_votes" integer,
	-- Stability: |correlation| of claim positions on the main axis between two halves of the voters.
	"split_correlation" double precision,
	"split_claims" integer,
	-- Everything else the run measured (per-k validation scores, spreads), for diagnosis.
	"detail" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"gate_met" boolean NOT NULL,
	-- Which gate conditions failed, e.g. ["clean_votes", "split_correlation"].
	"gate_failures" text[] NOT NULL,
	"elapsed_ms" integer
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "stance_map_runs_ran_at_idx" ON "personalization"."stance_map_runs" ("ran_at" DESC);
--> statement-breakpoint

-- The observations the map is fitted on. `source` is 'vote' or 'stated'; `weight` is already the
-- account weight (times stated_weight for stated sides). One row per (user, claim, source).
CREATE OR REPLACE FUNCTION personalization.stance_map_observations(as_of timestamptz DEFAULT now())
RETURNS TABLE (user_id uuid, claim_id uuid, agree boolean, weight double precision, source text)
LANGUAGE sql STABLE AS $$
	WITH claims AS (
		SELECT DISTINCT r.from_entity_id AS claim_id
		FROM public.relations r
		WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
		  AND r.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid -- Claim
	),
	latest AS (
		SELECT DISTINCT ON (uv.user_id, uv.object_id) uv.user_id, uv.object_id, uv.vote_type
		FROM public.user_votes uv
		JOIN claims c ON c.claim_id = uv.object_id
		WHERE uv.vote_kind = 1 AND uv.vote_type IN (0, 1) AND uv.voted_at <= as_of
		ORDER BY uv.user_id, uv.object_id, uv.voted_at DESC
	),
	debates AS (
		SELECT t.from_entity_id AS debate_id
		FROM public.relations t
		WHERE t.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid       -- Types
		  AND t.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid  -- Debate
		GROUP BY t.from_entity_id
	),
	main_claim AS (
		-- Exactly one main claim; a topic debate has none and states nothing.
		SELECT c.from_entity_id AS debate_id, min(c.to_entity_id::text)::uuid AS claim_id
		FROM public.relations c
		JOIN debates d ON d.debate_id = c.from_entity_id
		WHERE c.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid       -- Claims
		GROUP BY c.from_entity_id
		HAVING count(DISTINCT c.to_entity_id) = 1
	),
	sides AS (
		SELECT DISTINCT s.from_entity_id AS debate_id, s.to_entity_id AS user_id,
		       (s.type_id = 'd19fad56-5136-4a7f-8309-daf5c7bf99dd'::uuid) AS agree   -- Supported by
		FROM public.relations s
		JOIN main_claim m ON m.debate_id = s.from_entity_id
		WHERE s.type_id IN ('d19fad56-5136-4a7f-8309-daf5c7bf99dd'::uuid,         -- Supported by
		                    'c57de77c-3eee-4e7b-a0d2-258d18aab11c'::uuid)         -- Opposed by
	),
	stated AS (
		-- A user may have argued the same claim more than once; keep the side they argued more
		-- often, and drop the claim for them if it is a tie (including arguing both sides of one).
		SELECT sd.user_id, m.claim_id,
		       count(*) FILTER (WHERE sd.agree) > count(*) FILTER (WHERE NOT sd.agree) AS agree
		FROM sides sd
		JOIN main_claim m ON m.debate_id = sd.debate_id
		JOIN public.entities e ON e.id = sd.debate_id
		-- A debate with an unreadable creation time is kept: it exists now, and as_of is for
		-- reproducing a past run, not a security boundary.
		WHERE coalesce(personalization.epoch_text_ts(e.created_at) <= as_of, true)
		GROUP BY sd.user_id, m.claim_id
		HAVING count(*) FILTER (WHERE sd.agree) <> count(*) FILTER (WHERE NOT sd.agree)
	)
	SELECT l.user_id, l.object_id, l.vote_type = 0, w.weight, 'vote'
	FROM latest l
	JOIN public.account_weights w ON w.user_id = l.user_id
	WHERE w.weight > 0
	UNION ALL
	SELECT s.user_id, s.claim_id, s.agree, w.weight * cfg.stated_weight, 'stated'
	FROM stated s
	JOIN public.account_weights w ON w.user_id = s.user_id
	CROSS JOIN personalization.stance_map_config cfg
	WHERE w.weight > 0 AND cfg.stated_weight > 0
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.stance_map_observations(timestamptz) IS E'@omit';
--> statement-breakpoint

-- Claim-stance seeds: +1 when `claim_id` Supports `main_claim_id`, -1 when it Opposes it. A pair
-- with both relations (in different spaces, say) contradicts itself and is dropped.
CREATE OR REPLACE FUNCTION personalization.stance_map_seeds()
RETURNS TABLE (claim_id uuid, main_claim_id uuid, sign smallint)
LANGUAGE sql STABLE AS $$
	SELECT r.from_entity_id, r.to_entity_id,
	       max(CASE WHEN r.type_id = '81faa4ad-afad-4009-b106-1374c1219f04'::uuid THEN 1 ELSE -1 END)::smallint
	FROM public.relations r
	WHERE r.type_id IN ('81faa4ad-afad-4009-b106-1374c1219f04'::uuid,   -- Supports
	                    '71d1bcd5-f1cf-487f-b9a3-c7b1d53803eb'::uuid)   -- Opposes
	  AND r.from_entity_id <> r.to_entity_id
	GROUP BY r.from_entity_id, r.to_entity_id
	HAVING count(DISTINCT r.type_id) = 1
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.stance_map_seeds() IS E'@omit';
--> statement-breakpoint

-- Records one run and decides the gate from stance_map_config, so the thresholds live in one
-- place. `m` carries the run's metrics by column name (see stance_map_runs); unknown keys are
-- kept in `detail`. Returns the new row's gate_met. A NULL score fails its gate condition: a run
-- that could not measure stability has not shown it.
CREATE OR REPLACE FUNCTION personalization.record_stance_map_run(m jsonb)
RETURNS boolean
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
	cfg personalization.stance_map_config%ROWTYPE;
	failures text[] := '{}';
	met boolean;
BEGIN
	IF m IS NULL OR jsonb_typeof(m) <> 'object' OR NOT (m ? 'model_version') OR NOT (m ? 'clean_votes') THEN
		RAISE EXCEPTION 'record_stance_map_run: metrics must be an object with model_version and clean_votes'
			USING ERRCODE = '22023';
	END IF;
	SELECT * INTO cfg FROM personalization.stance_map_config WHERE id;

	IF (m ->> 'clean_votes')::integer < cfg.min_clean_votes THEN
		failures := failures || 'clean_votes'::text;
	END IF;
	IF ((m ->> 'split_correlation')::double precision >= cfg.min_split_correlation) IS NOT TRUE THEN
		failures := failures || 'split_correlation'::text;
	END IF;
	IF ((m ->> 'auc_lift')::double precision > cfg.min_auc_lift) IS NOT TRUE THEN
		failures := failures || 'auc_lift'::text;
	END IF;
	met := cardinality(failures) = 0;

	INSERT INTO personalization.stance_map_runs
		(model_version, clean_votes, weighted_votes, voters, claims, stated_positions, seeded_claims,
		 axes, lambda, auc_map, auc_tendencies, auc_lift, auc_map_unseeded, test_votes,
		 split_correlation, split_claims, detail, gate_met, gate_failures, elapsed_ms)
	VALUES (
		m ->> 'model_version',
		(m ->> 'clean_votes')::integer,
		coalesce((m ->> 'weighted_votes')::double precision, 0),
		coalesce((m ->> 'voters')::integer, 0),
		coalesce((m ->> 'claims')::integer, 0),
		coalesce((m ->> 'stated_positions')::integer, 0),
		coalesce((m ->> 'seeded_claims')::integer, 0),
		(m ->> 'axes')::integer,
		(m ->> 'lambda')::double precision,
		(m ->> 'auc_map')::double precision,
		(m ->> 'auc_tendencies')::double precision,
		(m ->> 'auc_lift')::double precision,
		(m ->> 'auc_map_unseeded')::double precision,
		(m ->> 'test_votes')::integer,
		(m ->> 'split_correlation')::double precision,
		(m ->> 'split_claims')::integer,
		coalesce(m -> 'detail', '{}'::jsonb),
		met,
		failures,
		(m ->> 'elapsed_ms')::integer
	);
	RETURN met;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.record_stance_map_run(jsonb) IS E'@omit';
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA personalization TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON personalization.stance_map_config, personalization.stance_map_runs TO gaia_app';
    EXECUTE 'GRANT USAGE ON SEQUENCE personalization.stance_map_runs_id_seq TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
