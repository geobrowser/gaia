-- GEO-3140 / GEO-3144: serve For you, versioned, and compare feed versions by interleaving.
--
-- For you is Best's candidate set re-ordered by one user's topic interests (GEO-3088). The web
-- app's server fetches Best's candidate window through the public, shared, cached GraphQL query as
-- it does today, then asks the api's private `/internal/for-you` route to re-order that window for
-- the signed-in user. Nothing per-user is ever served through GraphQL, so the response cache (keyed
-- on the query) can never hand one user's page to another.
--
-- Everything here lives in the private `personalization` schema, which PostGraphile does not
-- introspect, and is listed in HIDDEN_SURFACE (api/src/kg/securitySignals.ts).
--
-- THE MAPPING FROM INTEREST UNITS TO RANKING-SCORE UNITS. Interest weights are decayed, weighted
-- counts (a fresh vote on a topic is 1.0, a follow 3.0). Best's ranking_score is log-quality plus
-- structure plus participation plus created_at / tau, so one day of recency is 86400 / tau units
-- (0.864 at tau = 100000). The boost is expressed in DAYS OF RECENCY and saturates:
--
--     boost = max_boost_days * (86400 / tau) * w / (w + interest_half_saturation)
--
-- where w is the user's weight on the item's most-weighted topic (max, not sum: an item tagged
-- with five topics the user likes is not five times as relevant). It is bounded, so no amount of
-- activity on one topic can push an item past max_boost_days of recency, and it is expressed in
-- days so it survives a change to tau.
--
-- Calibrated 2026-10-05 against real data: Best's first window on www.geobrowser.io (66 Debate and
-- Claim candidates, which spans 8.9 score units, about 10 days of recency; rank 22 to rank 66 is
-- 3.4 units) and every public vote (11,937 rows, 208 voters with weights, 162 of them with at
-- least one candidate on their topics), re-ranking within the window as the route does:
--
--     max_boost_days  half_sat  page-1 items changed  users unchanged  interest items on page 1  page 1 >= half one topic
--          1             3            0.6                 87 / 162          7.5 -> 8.0               0
--          3             3            1.1                 67 / 162          7.5 -> 8.5               0
--          6             2            2.2                 42 / 162          7.5 -> 9.5               2
--         10             1            4.4                 10 / 162          7.5 -> 11.6              8   (37 all-interest pages)
--
-- 1 day is too weak (most users see Best unchanged); 10 days with a half-saturation of one vote is
-- too strong (one topic takes half of page one for 8 users, and 37 get a page with nothing outside
-- their interests). 6 days at 2 is the conservative middle. That run approximated interest from
-- votes alone; real weights also count comments, debates and follows (which do not decay), so
-- live users sit somewhat higher on the curve. Re-ranking within Best's window is itself a cap:
-- only the 66 candidates Best would have served on its first three pages can reach page one.
--
-- VERSIONS. Every For you page names `for-you-<code version>.<revision>`. The code version is a
-- constant in the api; the revision below increments on ANY change to the tunables that shape the
-- page: this file's config, the GEO-3088 interest weights and half-life, and Best's own ranking
-- config (For you's candidates and base scores are Best's). A no-op UPDATE also bumps it, which is
-- the safe direction: a version must never claim to be the same feed when it might not be.

CREATE TABLE IF NOT EXISTS "personalization"."for_you_config" (
  "id" boolean PRIMARY KEY DEFAULT true CHECK ("id"),
  -- The most an interest can be worth, in days of recency. See the header.
  "max_boost_days" double precision NOT NULL DEFAULT 6 CHECK ("max_boost_days" >= 0),
  -- The interest weight at which an item gets half the maximum boost.
  "interest_half_saturation" double precision NOT NULL DEFAULT 2 CHECK ("interest_half_saturation" > 0),
  -- Share of a page's slots reserved for items outside the user's interests, each marked as
  -- exploration with the probability it had of being picked.
  "exploration_share" double precision NOT NULL DEFAULT 0.10
    CHECK ("exploration_share" >= 0 AND "exploration_share" <= 0.5),
  -- How many of the user's topics are read per request (user_topic_weights' max_topics).
  "max_topics" integer NOT NULL DEFAULT 50 CHECK ("max_topics" > 0)
);
--> statement-breakpoint
INSERT INTO "personalization"."for_you_config" ("id") VALUES (true) ON CONFLICT DO NOTHING;
--> statement-breakpoint

-- One row. `revision` is part of every For you version string.
CREATE TABLE IF NOT EXISTS "personalization"."feed_config_revision" (
  "id" boolean PRIMARY KEY DEFAULT true CHECK ("id"),
  "revision" integer NOT NULL DEFAULT 0,
  "changed_at" timestamptz NOT NULL DEFAULT now(),
  "changed_table" text
);
--> statement-breakpoint
INSERT INTO "personalization"."feed_config_revision" ("id") VALUES (true) ON CONFLICT DO NOTHING;
--> statement-breakpoint

CREATE OR REPLACE FUNCTION personalization.bump_feed_config_revision()
RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  UPDATE personalization.feed_config_revision
     SET revision = revision + 1, changed_at = now(), changed_table = TG_TABLE_SCHEMA || '.' || TG_TABLE_NAME
   WHERE id;
  RETURN NULL;
END;
$$;
--> statement-breakpoint

DROP TRIGGER IF EXISTS for_you_config_revision ON personalization.for_you_config;
--> statement-breakpoint
CREATE TRIGGER for_you_config_revision
  AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON personalization.for_you_config
  FOR EACH STATEMENT EXECUTE FUNCTION personalization.bump_feed_config_revision();
--> statement-breakpoint
DROP TRIGGER IF EXISTS interest_signal_weights_revision ON personalization.interest_signal_weights;
--> statement-breakpoint
CREATE TRIGGER interest_signal_weights_revision
  AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON personalization.interest_signal_weights
  FOR EACH STATEMENT EXECUTE FUNCTION personalization.bump_feed_config_revision();
--> statement-breakpoint
DROP TRIGGER IF EXISTS interest_config_revision ON personalization.interest_config;
--> statement-breakpoint
CREATE TRIGGER interest_config_revision
  AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON personalization.interest_config
  FOR EACH STATEMENT EXECUTE FUNCTION personalization.bump_feed_config_revision();
--> statement-breakpoint
DROP TRIGGER IF EXISTS entity_ranking_config_feed_revision ON public.entity_ranking_config;
--> statement-breakpoint
CREATE TRIGGER entity_ranking_config_feed_revision
  AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.entity_ranking_config
  FOR EACH STATEMENT EXECUTE FUNCTION personalization.bump_feed_config_revision();
--> statement-breakpoint

-- Everything the re-rank needs about each candidate, in the order given: its Best score, its
-- topics, and whether the user has already answered it. Already answered means a position still
-- held on it (a stance vote, agree or disagree, as geogenesis' POSITION_VOTE_KINDS/TYPES), or
-- Interested on a question (GEO-3088's hook table; nothing writes it yet). Those are excluded from
-- For you, as the votedBy filter excludes them elsewhere.
--
-- One index probe per candidate on each side: relations_type_from_to_idx for topics,
-- user_votes_pkey's (user_id, object_id) prefix for votes, the hook table's primary key.
CREATE OR REPLACE FUNCTION personalization.for_you_candidates(p_user_id uuid, p_candidate_ids uuid[])
RETURNS TABLE (entity_id uuid, ranking_score double precision, topic_ids uuid[], excluded text)
LANGUAGE sql STABLE AS $$
  SELECT c.id,
         ers.ranking_score::double precision,
         coalesce((
           SELECT array_agg(DISTINCT r.to_entity_id ORDER BY r.to_entity_id)
           FROM public.relations r
           WHERE r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid  -- Topics
             AND r.from_entity_id = c.id
         ), '{}'::uuid[]),
         CASE
           WHEN EXISTS (
             SELECT 1 FROM public.user_votes uv
             WHERE uv.user_id = p_user_id AND uv.object_id = c.id
               AND uv.vote_kind = 1 AND uv.vote_type IN (0, 1)
           ) THEN 'voted'
           WHEN EXISTS (
             SELECT 1 FROM personalization.external_interest_signals s
             WHERE s.user_id = p_user_id AND s.object_id = c.id AND s.kind = 'interested'
           ) THEN 'interested'
         END
  FROM unnest(p_candidate_ids) WITH ORDINALITY AS c(id, ord)
  LEFT JOIN public.entity_ranking_scores ers ON ers.entity_id = c.id
  ORDER BY c.ord
$$;
--> statement-breakpoint

-- GEO-3144. Feed experiments: two named feed versions interleaved into one page for a group of
-- users. Changing which versions are compared, or who is in the group, is a row change here, not a
-- deploy. At most one experiment is active at a time.
CREATE TABLE IF NOT EXISTS "personalization"."feed_experiments" (
  "id" text PRIMARY KEY,
  "active" boolean NOT NULL DEFAULT false,
  -- Feed families the web app can build. The served version string (e.g. best-1, for-you-1.0)
  -- travels with each card, so a version bump mid-experiment is visible in the data.
  "arm_a" text NOT NULL CHECK ("arm_a" IN ('best', 'for-you')),
  "arm_b" text NOT NULL CHECK ("arm_b" IN ('best', 'for-you')),
  -- Fraction of signed-in users hashed into the interleaved group, on top of explicit members.
  "share" double precision NOT NULL DEFAULT 0 CHECK ("share" >= 0 AND "share" <= 1),
  -- Changing the salt reshuffles who the hash puts in the group; leave it alone mid-experiment.
  "salt" text NOT NULL DEFAULT md5(random()::text),
  "note" text,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "feed_experiments_one_active"
  ON "personalization"."feed_experiments" ("active") WHERE "active";
--> statement-breakpoint

-- Explicit membership, which wins over the hash either way: the internal group first, a cohort
-- later, and anyone who must be kept out.
CREATE TABLE IF NOT EXISTS "personalization"."feed_experiment_members" (
  "experiment_id" text NOT NULL REFERENCES "personalization"."feed_experiments" ("id") ON DELETE CASCADE,
  "user_id" uuid NOT NULL,
  "interleaved" boolean NOT NULL DEFAULT true,
  "added_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("experiment_id", "user_id")
);
--> statement-breakpoint

-- The active experiment and whether this user is in its interleaved group. No row when nothing is
-- active. Deterministic in (salt, user), so a user stays in the same group across sessions.
CREATE OR REPLACE FUNCTION personalization.feed_experiment_for_user(p_user_id uuid)
RETURNS TABLE (experiment_id text, arm_a text, arm_b text, interleaved boolean, assignment text)
LANGUAGE sql STABLE AS $$
  SELECT x.id, x.arm_a, x.arm_b,
         coalesce(m.interleaved,
                  ('x' || substr(md5(x.salt || ':' || p_user_id::text), 1, 8))::bit(32)::bigint / 4294967296.0 < x.share),
         CASE WHEN m.user_id IS NOT NULL THEN 'member' ELSE 'hash' END
  FROM personalization.feed_experiments x
  LEFT JOIN personalization.feed_experiment_members m ON m.experiment_id = x.id AND m.user_id = p_user_id
  WHERE x.active
$$;
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA personalization TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA personalization TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
