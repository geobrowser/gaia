-- GEO-3143: choose the primer and anchor claims that teach us the most about a user.
--
-- Votes only place a user if other people voted on the same claims, and on 2 Oct 2026 the median
-- pair of users shared none. Two things are chosen on purpose here:
--
--   * the 5-claim PRIMER, which gives each new user a starting point (`next_primer_claim`);
--   * a shared set of 30-50 ANCHOR claims that everyone is nudged towards (`anchor_claims`).
--
-- INFORMATION, NOT POPULARITY. A claim almost everyone agrees with tells us almost nothing about
-- the person answering it. A claim is worth asking in proportion to how evenly it splits the
-- people who answered it (the binary entropy of its agree share, in bits: 1.0 at 50/50, 0.47 at
-- 90/10, 0 when unanimous) and how many people answered it (ln(1 + voters): each extra voter is
-- another person a new answer can be compared with, with diminishing returns). v1 scores a claim
-- as the product of the two, and the primer adds a bonus for topics the user has not answered
-- yet. Once the stance map is stable, switch to the claim whose answer is expected to tell us the
-- most about the user's position (expected information gain against the map).
--
-- WEIGHTED. Every count below is a sum of `account_vote_weight(voter)` (0101, GEO-3141), so test
-- and junk accounts (weight 0), brand-new accounts (ramping up) and one-sided voters (x0.25)
-- cannot make a claim look contested or popular. The raw voter count is kept alongside.
--
-- PRIVATE STATE, PUBLIC CHOICES. The per-claim statistics, the anchor history and the overlap
-- samples are @omit and listed in HIDDEN_SURFACE (api/src/kg/securitySignals.ts): they are built
-- from account weights, which must not be readable. What the client needs is public and
-- returns entities only, never a score: `anchorClaims` (the current set, or the set in force at
-- a given time) and `nextPrimerClaim` (one claim for a user). Neither is a per-user secret: an
-- anchor set is the same for everyone, and the primer reads only the user's own public votes and
-- the arguments the client sends.
--
-- REFRESH. ranking-indexer's `primer_claims_refresh` CronJob runs hourly. It rebuilds the
-- statistics every run, starts a new anchor set only on its first run in a new calendar month
-- (so the anchors are refreshed monthly, and every past set is kept), and records the pair
-- overlap at most once a day.

-- One row per Claim that at least one weighted voter has taken a stance on (Agree/Disagree,
-- vote_kind 1). Rebuilt in full by refresh_primer_claim_stats().
CREATE TABLE IF NOT EXISTS "primer_claim_stats" (
	"claim_id" uuid PRIMARY KEY NOT NULL,
	-- Spaces in which the entity is typed Claim: the spaces a user can pick it from.
	"space_ids" uuid[] NOT NULL,
	-- Topics (the Topics property), from any space.
	"topic_ids" uuid[] NOT NULL,
	-- Distinct stance voters, unweighted, for reporting.
	"voters" integer NOT NULL,
	"weighted_voters" double precision NOT NULL,
	"weighted_agree" double precision NOT NULL,
	-- weighted_agree / weighted_voters.
	"agree_share" double precision NOT NULL,
	-- Binary entropy (bits) of the smoothed agree share (weighted_agree + 1) / (weighted_voters + 2),
	-- so a claim with one voter is not mistaken for a perfectly split one.
	"information" double precision NOT NULL,
	-- information * ln(1 + weighted_voters): what one answer to this claim is worth.
	"score" double precision NOT NULL,
	-- agree_share within [contested_low, contested_high] with at least min_voters weighted voters.
	"contested" boolean NOT NULL,
	-- Weighted distinct voters of any kind (curation, stance, veracity): how much people engage.
	"engagement" double precision NOT NULL,
	"name_length" integer NOT NULL,
	"computed_at" timestamp with time zone NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "primer_claim_stats_space_ids_idx" ON "primer_claim_stats" USING gin ("space_ids");
--> statement-breakpoint
COMMENT ON TABLE "primer_claim_stats" IS E'@omit';
--> statement-breakpoint

-- Every anchor set ever chosen. The set in force at time T is the newest with effective_from <= T,
-- which is what makes "which claims were anchors on a given date" recoverable.
CREATE TABLE IF NOT EXISTS "anchor_claim_sets" (
	"id" serial PRIMARY KEY NOT NULL,
	"effective_from" timestamp with time zone NOT NULL,
	"claim_count" integer NOT NULL,
	-- The parameters the set was chosen with, so a later retune can be told apart.
	"params" jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "anchor_claim_sets_effective_idx" ON "anchor_claim_sets" ("effective_from");
--> statement-breakpoint
COMMENT ON TABLE "anchor_claim_sets" IS E'@omit';
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "anchor_claim_members" (
	"set_id" integer NOT NULL REFERENCES "anchor_claim_sets" ("id") ON DELETE CASCADE,
	"claim_id" uuid NOT NULL,
	"rank" integer NOT NULL,
	-- The statistics the claim was chosen on, frozen at selection time.
	"score" double precision NOT NULL,
	"weighted_voters" double precision NOT NULL,
	"agree_share" double precision NOT NULL,
	PRIMARY KEY ("set_id", "claim_id")
);
--> statement-breakpoint
COMMENT ON TABLE "anchor_claim_members" IS E'@omit';
--> statement-breakpoint

-- The ticket's outcome measure, sampled daily: of the pairs of counted users (weight > 0, at
-- least min_votes stance votes on claims), the share that have both taken a stance on at least
-- min_shared of the same claims. The 2 Oct 2026 baseline was 26.5% at (5, 3).
CREATE TABLE IF NOT EXISTS "claim_overlap_samples" (
	"sampled_at" timestamp with time zone PRIMARY KEY NOT NULL,
	"min_votes" integer NOT NULL,
	"min_shared" integer NOT NULL,
	"users" integer NOT NULL,
	"pairs" bigint NOT NULL,
	"pairs_sharing" bigint NOT NULL,
	"share" double precision,
	"anchor_set_id" integer
);
--> statement-breakpoint
COMMENT ON TABLE "claim_overlap_samples" IS E'@omit';
--> statement-breakpoint

-- Binary entropy in bits. 0 at p = 0 or 1, 1 at p = 0.5.
CREATE OR REPLACE FUNCTION public.binary_entropy_bits(p double precision)
RETURNS double precision
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
	SELECT CASE WHEN p IS NULL THEN NULL
	            WHEN p <= 0 OR p >= 1 THEN 0.0
	            ELSE -(p * ln(p) + (1 - p) * ln(1 - p)) / ln(2.0) END
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.binary_entropy_bits(double precision) IS E'@omit';
--> statement-breakpoint

-- Rebuilds primer_claim_stats. DELETE then INSERT in one function call, so one transaction:
-- readers see the old statistics or the new ones, never a half-built table. Returns rows written.
--
-- A claim is included when it is typed Claim, could appear in a feed (named, not System, no
-- excluded type: entity_topic_ranking_eligible) and has a weighted stance voter. A user's stance
-- on a claim is their most recent stance vote on it in any space.
CREATE OR REPLACE FUNCTION public.refresh_primer_claim_stats(
	min_voters double precision DEFAULT 3,
	contested_low double precision DEFAULT 0.25,
	contested_high double precision DEFAULT 0.75,
	as_of timestamp with time zone DEFAULT now()
)
RETURNS integer
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
	written integer;
BEGIN
	IF min_voters < 0 OR contested_low < 0 OR contested_high > 1 OR contested_low > contested_high THEN
		RAISE EXCEPTION 'refresh_primer_claim_stats: invalid parameters' USING ERRCODE = '22023';
	END IF;

	DELETE FROM public.primer_claim_stats;

	INSERT INTO public.primer_claim_stats
		(claim_id, space_ids, topic_ids, voters, weighted_voters, weighted_agree, agree_share,
		 information, score, contested, engagement, name_length, computed_at)
	WITH claims AS (
		SELECT r.from_entity_id AS claim_id, array_agg(DISTINCT r.space_id) AS space_ids
		FROM public.relations r
		WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
		  AND r.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid -- Claim
		GROUP BY r.from_entity_id
	),
	stances AS (
		SELECT DISTINCT ON (uv.user_id, uv.object_id) uv.user_id, uv.object_id, uv.vote_type
		FROM public.user_votes uv
		JOIN claims c ON c.claim_id = uv.object_id
		WHERE uv.vote_kind = 1 AND uv.vote_type IN (0, 1) AND uv.voted_at <= as_of
		ORDER BY uv.user_id, uv.object_id, uv.voted_at DESC
	),
	stance_totals AS (
		SELECT s.object_id AS claim_id,
		       count(*)::integer AS voters,
		       sum(coalesce(w.weight, 0)) AS weighted_voters,
		       sum(coalesce(w.weight, 0)) FILTER (WHERE s.vote_type = 0) AS weighted_agree
		FROM stances s
		LEFT JOIN public.account_weights w ON w.user_id = s.user_id
		GROUP BY s.object_id
	),
	engaged AS (
		SELECT e.object_id AS claim_id, sum(coalesce(w.weight, 0)) AS engagement
		FROM (SELECT DISTINCT uv.user_id, uv.object_id
		      FROM public.user_votes uv
		      JOIN stance_totals t ON t.claim_id = uv.object_id
		      WHERE uv.voted_at <= as_of) e
		LEFT JOIN public.account_weights w ON w.user_id = e.user_id
		GROUP BY e.object_id
	),
	measured AS (
		SELECT t.claim_id, t.voters, t.weighted_voters,
		       coalesce(t.weighted_agree, 0) AS weighted_agree,
		       coalesce(t.weighted_agree, 0) / t.weighted_voters AS agree_share,
		       public.binary_entropy_bits((coalesce(t.weighted_agree, 0) + 1) / (t.weighted_voters + 2)) AS information
		FROM stance_totals t
		WHERE t.weighted_voters > 0
	)
	SELECT m.claim_id,
	       c.space_ids,
	       coalesce((SELECT array_agg(DISTINCT tr.to_entity_id) FROM public.relations tr
	                 WHERE tr.from_entity_id = m.claim_id
	                   AND tr.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid), '{}'::uuid[]),
	       m.voters, m.weighted_voters, m.weighted_agree, m.agree_share, m.information,
	       m.information * ln(1 + m.weighted_voters),
	       m.weighted_voters >= min_voters AND m.agree_share BETWEEN contested_low AND contested_high,
	       coalesce(e.engagement, m.weighted_voters),
	       coalesce((SELECT max(length(btrim(v.text))) FROM public.values v
	                 WHERE v.entity_id = m.claim_id
	                   AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid), 0),
	       as_of
	FROM measured m
	JOIN claims c ON c.claim_id = m.claim_id
	LEFT JOIN engaged e ON e.claim_id = m.claim_id
	WHERE public.entity_topic_ranking_eligible(m.claim_id);

	GET DIAGNOSTICS written = ROW_COUNT;
	RETURN written;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_primer_claim_stats(double precision, double precision, double precision, timestamp with time zone) IS E'@omit';
--> statement-breakpoint

-- The anchor set in force at `as_of`: the newest set that took effect at or before it.
CREATE OR REPLACE FUNCTION public.anchor_claim_set_at(as_of timestamp with time zone DEFAULT now())
RETURNS integer
LANGUAGE sql STABLE PARALLEL SAFE AS $$
	SELECT id FROM public.anchor_claim_sets
	WHERE effective_from <= coalesce(as_of, now())
	ORDER BY effective_from DESC
	LIMIT 1
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.anchor_claim_set_at(timestamp with time zone) IS E'@omit';
--> statement-breakpoint

-- Chooses a new anchor set from primer_claim_stats, unless the newest set already took effect in
-- the same calendar month (UTC) as `as_of` and `force` is false. Returns the new set's id, or NULL
-- when it did nothing. Past sets are never modified.
--
-- Selection, best score first (information x ln(1 + voters), i.e. the most voters and the closest
-- agree/disagree split):
--   1. contested claims with at least min_voters weighted voters, at most max_per_topic anchors
--      sharing any one topic, so one hot topic cannot take the whole set;
--   2. if that gives fewer than min_size, the same claims without the topic cap;
--   3. if still short, the best remaining claims with at least min_voters weighted voters,
--      contested or not.
-- Stops at set_size. A set smaller than min_size is still stored (thin data is not a reason to
-- keep last month's anchors), and the caller logs it.
CREATE OR REPLACE FUNCTION public.refresh_anchor_claims(
	set_size integer DEFAULT 40,
	min_size integer DEFAULT 30,
	min_voters double precision DEFAULT 5,
	max_per_topic integer DEFAULT 4,
	force boolean DEFAULT false,
	as_of timestamp with time zone DEFAULT now()
)
RETURNS integer
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
	latest timestamp with time zone;
	new_id integer;
	chosen uuid[] := '{}';
	topic_counts jsonb := '{}'::jsonb;
	cand record;
	t uuid;
	blocked boolean;
BEGIN
	IF set_size < 1 OR min_size < 0 OR min_size > set_size OR min_voters < 0 OR max_per_topic < 1 THEN
		RAISE EXCEPTION 'refresh_anchor_claims: invalid parameters' USING ERRCODE = '22023';
	END IF;

	SELECT max(effective_from) INTO latest FROM public.anchor_claim_sets;
	IF NOT force AND latest IS NOT NULL
	   AND date_trunc('month', latest AT TIME ZONE 'UTC') = date_trunc('month', as_of AT TIME ZONE 'UTC') THEN
		RETURN NULL;
	END IF;
	IF latest IS NOT NULL AND as_of <= latest THEN
		RAISE EXCEPTION 'refresh_anchor_claims: as_of % is not after the newest set (%)', as_of, latest
			USING ERRCODE = '22023';
	END IF;

	-- 1. Contested, topic-capped.
	FOR cand IN
		SELECT s.claim_id, s.topic_ids FROM public.primer_claim_stats s
		WHERE s.contested AND s.weighted_voters >= min_voters
		ORDER BY s.score DESC, s.weighted_voters DESC, s.claim_id
	LOOP
		EXIT WHEN cardinality(chosen) >= set_size;
		blocked := false;
		FOREACH t IN ARRAY cand.topic_ids LOOP
			IF coalesce((topic_counts ->> t::text)::integer, 0) >= max_per_topic THEN blocked := true; END IF;
		END LOOP;
		CONTINUE WHEN blocked;
		chosen := chosen || cand.claim_id;
		FOREACH t IN ARRAY cand.topic_ids LOOP
			topic_counts := jsonb_set(topic_counts, ARRAY[t::text], to_jsonb(coalesce((topic_counts ->> t::text)::integer, 0) + 1));
		END LOOP;
	END LOOP;

	-- 2. Contested, uncapped. 3. Any claim with enough voters.
	IF cardinality(chosen) < min_size THEN
		chosen := chosen || ARRAY(
			SELECT s.claim_id FROM public.primer_claim_stats s
			WHERE s.contested AND s.weighted_voters >= min_voters AND s.claim_id <> ALL (chosen)
			ORDER BY s.score DESC, s.weighted_voters DESC, s.claim_id
			LIMIT greatest(0, min_size - cardinality(chosen)));
	END IF;
	IF cardinality(chosen) < min_size THEN
		chosen := chosen || ARRAY(
			SELECT s.claim_id FROM public.primer_claim_stats s
			WHERE s.weighted_voters >= min_voters AND s.claim_id <> ALL (chosen)
			ORDER BY s.score DESC, s.weighted_voters DESC, s.claim_id
			LIMIT greatest(0, min_size - cardinality(chosen)));
	END IF;

	INSERT INTO public.anchor_claim_sets (effective_from, claim_count, params)
	VALUES (as_of, cardinality(chosen), jsonb_build_object(
		'set_size', set_size, 'min_size', min_size, 'min_voters', min_voters,
		'max_per_topic', max_per_topic, 'forced', force))
	RETURNING id INTO new_id;

	INSERT INTO public.anchor_claim_members (set_id, claim_id, rank, score, weighted_voters, agree_share)
	SELECT new_id, s.claim_id, c.ord::integer, s.score, s.weighted_voters, s.agree_share
	FROM unnest(chosen) WITH ORDINALITY AS c(claim_id, ord)
	JOIN public.primer_claim_stats s ON s.claim_id = c.claim_id;

	RETURN new_id;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_anchor_claims(integer, integer, double precision, integer, boolean, timestamp with time zone) IS E'@omit';
--> statement-breakpoint

-- Every candidate for a user's next primer claim, best first, with why. The selection itself;
-- next_primer_claim returns its first row as an entity. Internal (it exposes scores).
--
-- Arguments, all nullable:
--   user_id            the user's personal space id (user_votes.user_id). Claims they have
--                      already taken a stance on are never offered, and their topics count as
--                      answered. NULL for a user who has not voted yet.
--   space_ids          the spaces they picked in onboarding, top space first.
--   answered_claim_ids the primer claims they have answered so far, in this primer. An answer
--                      takes a moment to reach user_votes, so the client sends them as well.
--   skipped_claim_ids  claims they skipped. gaia has no write path for a skip, so the client keeps
--                      them; whatever it sends is never offered again.
--
-- Rules (GEO-3143's acceptance criteria):
--   * nothing answered, skipped or already voted on is returned; after primer_size answers,
--     nothing is returned at all;
--   * the first claim is the most engaging contested claim in the top picked space;
--   * later claims favour score x (1 + novelty_weight x share of the claim's topics the user has
--     not answered), so contested claims with many voters on new topics;
--   * at least min_anchors of the primer_size claims are anchors, and when 2+ spaces were picked
--     the answers span at least 2 of them. A candidate that would make either impossible in the
--     slots left is ranked below every candidate that keeps both possible;
--   * when the picked spaces hold fewer than pool_min eligible claims (or the user has used them
--     up), anchors on related topics fill in, then any anchor, then any eligible claim, so a user
--     is never left with fewer than primer_size to answer while claims exist.
-- An eligible claim is in primer_claim_stats with at least min_voters weighted stance voters and,
-- when max_name_chars is set, a name no longer than that.
CREATE OR REPLACE FUNCTION public.primer_candidates(
	user_id uuid,
	space_ids uuid[],
	answered_claim_ids uuid[],
	skipped_claim_ids uuid[],
	primer_size integer DEFAULT 5,
	min_anchors integer DEFAULT 2,
	pool_min integer DEFAULT 10,
	min_voters double precision DEFAULT 3,
	novelty_weight double precision DEFAULT 1.0,
	max_name_chars integer DEFAULT NULL
)
RETURNS TABLE (
	claim_id uuid,
	rank integer,
	is_anchor boolean,
	in_picked_space boolean,
	pool text,
	keeps_rules boolean,
	value double precision,
	novel_topic_share double precision,
	reason text
)
LANGUAGE plpgsql STABLE AS $$
DECLARE
	picked uuid[];
	answered uuid[];
	seen uuid[];
	answered_count integer;
	remaining integer;
	anchor_set integer;
	anchors_need integer;
	covered uuid[];
	spaces_need integer;
	answered_topics uuid[];
	related_topics uuid[];
	picked_pool integer;
	picked_unseen integer;
	wide boolean;
BEGIN
	-- Arguments keep their order (the top space is first) and lose duplicates and NULLs.
	picked := ARRAY(SELECT s FROM unnest(coalesce(space_ids, '{}')) WITH ORDINALITY AS a(s, o)
	                WHERE s IS NOT NULL GROUP BY s ORDER BY min(o));
	answered := ARRAY(SELECT DISTINCT a FROM unnest(coalesce(answered_claim_ids, '{}')) a WHERE a IS NOT NULL);
	answered_count := cardinality(answered);
	remaining := primer_size - answered_count;
	IF remaining <= 0 THEN
		RETURN;
	END IF;

	seen := answered
	     || ARRAY(SELECT DISTINCT a FROM unnest(coalesce(skipped_claim_ids, '{}')) a WHERE a IS NOT NULL)
	     || ARRAY(SELECT DISTINCT uv.object_id FROM public.user_votes uv
	              WHERE uv.user_id = primer_candidates.user_id AND uv.vote_kind = 1);

	anchor_set := public.anchor_claim_set_at(now());
	anchors_need := greatest(0, min_anchors - (
		SELECT count(*) FROM public.anchor_claim_members m
		WHERE m.set_id = anchor_set AND m.claim_id = ANY (answered)))::integer;

	covered := ARRAY(SELECT DISTINCT p FROM unnest(picked) p
	                 WHERE EXISTS (SELECT 1 FROM public.primer_claim_stats s
	                               WHERE s.claim_id = ANY (answered) AND p = ANY (s.space_ids)));

	answered_topics := ARRAY(SELECT DISTINCT t FROM public.primer_claim_stats s, unnest(s.topic_ids) t
	                         WHERE s.claim_id = ANY (seen) AND s.claim_id <> ALL (coalesce(skipped_claim_ids, '{}')));

	SELECT count(*), count(*) FILTER (WHERE s.claim_id <> ALL (seen))
	INTO picked_pool, picked_unseen
	FROM public.primer_claim_stats s
	WHERE s.space_ids && picked AND s.weighted_voters >= min_voters
	  AND (max_name_chars IS NULL OR s.name_length <= max_name_chars);
	wide := picked_pool < pool_min OR picked_unseen < remaining;

	-- Spaces still to cover: up to 2 picked spaces in total, counting only spaces that still have a
	-- claim to offer, so an exhausted space cannot make the rule unsatisfiable.
	spaces_need := CASE WHEN cardinality(picked) < 2 THEN 0 ELSE greatest(0, least(2,
		cardinality(covered) + (SELECT count(*) FROM unnest(picked) p
		                        WHERE p <> ALL (covered)
		                          AND EXISTS (SELECT 1 FROM public.primer_claim_stats s
		                                      WHERE p = ANY (s.space_ids) AND s.claim_id <> ALL (seen)
		                                        AND s.weighted_voters >= min_voters
		                                        AND (max_name_chars IS NULL OR s.name_length <= max_name_chars)))::integer)
		- cardinality(covered)) END;

	related_topics := ARRAY(
		SELECT DISTINCT t FROM public.primer_claim_stats s, unnest(s.topic_ids) t WHERE s.space_ids && picked
		UNION
		SELECT sp.topic_id FROM public.spaces sp WHERE sp.id = ANY (picked) AND sp.topic_id IS NOT NULL);

	RETURN QUERY
	WITH cands AS (
		SELECT s.claim_id, s.space_ids, s.topic_ids, s.score, s.contested, s.engagement,
		       (m.claim_id IS NOT NULL) AS is_anchor,
		       (s.space_ids && picked) AS in_picked,
		       (SELECT count(*) FROM unnest(picked) p WHERE p <> ALL (covered) AND p = ANY (s.space_ids))::integer AS new_spaces,
		       CASE WHEN cardinality(s.topic_ids) = 0 THEN 0.0
		            ELSE (SELECT count(*) FROM unnest(s.topic_ids) t WHERE t <> ALL (answered_topics))::double precision
		                 / cardinality(s.topic_ids) END AS novel
		FROM public.primer_claim_stats s
		LEFT JOIN public.anchor_claim_members m ON m.set_id = anchor_set AND m.claim_id = s.claim_id
		WHERE s.claim_id <> ALL (seen)
		  AND s.weighted_voters >= min_voters
		  AND (max_name_chars IS NULL OR s.name_length <= max_name_chars)
	),
	pooled AS (
		SELECT c.*,
		       CASE WHEN c.in_picked THEN 'picked_space'
		            WHEN c.is_anchor AND c.topic_ids && related_topics THEN 'related_anchor'
		            WHEN c.is_anchor THEN 'anchor'
		            ELSE 'any' END AS pool,
		       -- After taking this claim, can the remaining slots still meet both rules?
		       (greatest(0, anchors_need - c.is_anchor::integer)
		        + greatest(0, spaces_need - c.new_spaces)) <= remaining - 1 AS keeps_both,
		       greatest(greatest(0, anchors_need - c.is_anchor::integer),
		                greatest(0, spaces_need - c.new_spaces)) <= remaining - 1 AS keeps_one
		FROM cands c
		-- Anchors are always admissible (the primer needs them); other claims from outside the
		-- picked spaces only when the picked spaces run short, or nothing was picked.
		WHERE c.in_picked OR c.is_anchor OR wide OR cardinality(picked) = 0
	),
	valued AS (
		SELECT p.*,
		       (answered_count = 0 AND cardinality(picked) > 0 AND picked[1] = ANY (p.space_ids) AND p.contested) AS opener,
		       p.score * (1 + novelty_weight * p.novel) AS v
		FROM pooled p
	)
	SELECT v.claim_id,
	       (row_number() OVER w)::integer,
	       v.is_anchor,
	       v.in_picked,
	       v.pool,
	       v.keeps_both,
	       CASE WHEN v.opener THEN v.engagement ELSE v.v END,
	       v.novel,
	       CASE WHEN v.opener THEN 'most engaging contested claim in the top space'
	            WHEN NOT v.keeps_one THEN 'no candidate keeps the anchor and space rules possible'
	            WHEN NOT v.keeps_both THEN 'best available; the anchor and space rules may both not be met'
	            WHEN v.is_anchor AND anchors_need >= remaining THEN 'anchor needed'
	            WHEN v.new_spaces > 0 AND spaces_need >= remaining THEN 'second picked space needed'
	            ELSE 'most informative' END
	FROM valued v
	WINDOW w AS (
		ORDER BY v.keeps_both DESC, v.keeps_one DESC, v.opener DESC,
		         -- picked-space claims first; outside them, anchors on related topics, then any
		         -- anchor, then anything
		         CASE v.pool WHEN 'picked_space' THEN 0 WHEN 'related_anchor' THEN 1 WHEN 'anchor' THEN 2 ELSE 3 END,
		         CASE WHEN v.opener THEN v.engagement ELSE v.v END DESC,
		         v.claim_id)
	ORDER BY 2;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.primer_candidates(uuid, uuid[], uuid[], uuid[], integer, integer, integer, double precision, double precision, integer) IS E'@omit';
--> statement-breakpoint

-- The public read for the onboarding primer: the next claim to ask `user_id`, as an entity, or
-- no rows when the primer is complete (5 answered) or there is nothing left to ask. See
-- primer_candidates for the arguments and rules. GraphQL: `nextPrimerClaim`.
CREATE OR REPLACE FUNCTION public.next_primer_claim(
	user_id uuid,
	space_ids uuid[],
	answered_claim_ids uuid[],
	skipped_claim_ids uuid[]
)
RETURNS SETOF public.entities
LANGUAGE sql STABLE AS $$
	SELECT e.*
	FROM public.primer_candidates($1, $2, $3, $4) c
	JOIN public.entities e ON e.id = c.claim_id
	ORDER BY c.rank
	LIMIT 1
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.next_primer_claim(uuid, uuid[], uuid[], uuid[]) IS
	E'The next claim to ask a user in the onboarding primer (GEO-3143), or none once 5 are answered. Pass the spaces they picked (top first), the primer claims answered so far and any skipped.';
--> statement-breakpoint

-- The public read for anchors: the anchor claims in force at `as_of` (default now), best first.
-- GraphQL: `anchorClaims`.
CREATE OR REPLACE FUNCTION public.anchor_claims(as_of timestamp with time zone DEFAULT NULL)
RETURNS SETOF public.entities
LANGUAGE sql STABLE AS $$
	SELECT e.*
	FROM public.anchor_claim_members m
	JOIN public.entities e ON e.id = m.claim_id
	WHERE m.set_id = public.anchor_claim_set_at(coalesce(as_of, now()))
	ORDER BY m.rank
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.anchor_claims(timestamp with time zone) IS
	E'The shared anchor claims everyone is nudged towards (GEO-3143), best first: contested claims with many voters, refreshed monthly. Pass asOf for the set in force at an earlier time.';
--> statement-breakpoint

-- The ticket's outcome measure at `as_of`: among users with weight > 0 and at least min_votes
-- stance votes on claims cast by then, the share of pairs that took a stance on at least
-- min_shared of the same claims. Counts pairs through shared claims, so the cost grows with
-- the sum over claims of voters squared, not with users squared.
CREATE OR REPLACE FUNCTION public.claim_overlap_share(
	min_votes integer DEFAULT 5,
	min_shared integer DEFAULT 3,
	as_of timestamp with time zone DEFAULT now()
)
RETURNS TABLE (users integer, pairs bigint, pairs_sharing bigint, share double precision)
LANGUAGE sql STABLE AS $$
	WITH claims AS (
		SELECT DISTINCT r.from_entity_id AS claim_id FROM public.relations r
		WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
		  AND r.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid
	),
	stances AS (
		SELECT DISTINCT uv.user_id, uv.object_id
		FROM public.user_votes uv
		JOIN claims c ON c.claim_id = uv.object_id
		JOIN public.account_weights w ON w.user_id = uv.user_id AND w.weight > 0
		WHERE uv.vote_kind = 1 AND uv.vote_type IN (0, 1) AND uv.voted_at <= as_of
	),
	counted AS (
		SELECT user_id FROM stances GROUP BY user_id HAVING count(*) >= min_votes
	),
	kept AS (
		SELECT s.* FROM stances s JOIN counted USING (user_id)
	),
	shared AS (
		SELECT a.user_id AS u1, b.user_id AS u2
		FROM kept a JOIN kept b ON a.object_id = b.object_id AND a.user_id < b.user_id
		GROUP BY a.user_id, b.user_id
		HAVING count(*) >= min_shared
	)
	SELECT n::integer, n * (n - 1) / 2, s, CASE WHEN n > 1 THEN s::double precision / (n * (n - 1) / 2) END
	FROM (SELECT count(*)::bigint AS n FROM counted) u, (SELECT count(*)::bigint AS s FROM shared) p
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.claim_overlap_share(integer, integer, timestamp with time zone) IS E'@omit';
--> statement-breakpoint

-- Records claim_overlap_share once per UTC day (the CronJob runs hourly). Returns true when it
-- wrote a sample.
CREATE OR REPLACE FUNCTION public.record_claim_overlap_sample(
	min_votes integer DEFAULT 5,
	min_shared integer DEFAULT 3,
	as_of timestamp with time zone DEFAULT now()
)
RETURNS boolean
LANGUAGE plpgsql VOLATILE AS $$
BEGIN
	IF EXISTS (SELECT 1 FROM public.claim_overlap_samples s
	           WHERE (s.sampled_at AT TIME ZONE 'UTC')::date = (as_of AT TIME ZONE 'UTC')::date
	             AND s.min_votes = record_claim_overlap_sample.min_votes
	             AND s.min_shared = record_claim_overlap_sample.min_shared) THEN
		RETURN false;
	END IF;
	INSERT INTO public.claim_overlap_samples
		(sampled_at, min_votes, min_shared, users, pairs, pairs_sharing, share, anchor_set_id)
	SELECT as_of, min_votes, min_shared, o.users, o.pairs, o.pairs_sharing, o.share, public.anchor_claim_set_at(as_of)
	FROM public.claim_overlap_share(min_votes, min_shared, as_of) o;
	RETURN true;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.record_claim_overlap_sample(integer, integer, timestamp with time zone) IS E'@omit';
