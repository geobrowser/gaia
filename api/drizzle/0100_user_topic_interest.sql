-- GEO-3088: learn each user's topic interests from what they already do.
--
-- For you (GEO-3077) takes a list of topics with per-topic weights. This migration is the source
-- of those weights: a decayed, per-user score for every topic a user has engaged with, learned
-- from activity that is already in the graph. Serving it is GEO-3140; this is the data layer.
--
-- INPUTS, and how each is attributed to a user (= their personal space id, as in user_votes):
--
--   kind            source                                              user          when
--   vote            user_votes, entity votes that are not removals      user_id       voted_at
--   comment         relations Reply to (310d4a24), authored in a        space_id      the comment entity's
--                   Personal space; a reply to a comment walks up to                  created_at
--                   the thing the thread is about
--   debate          Debate entities (fd51f935): Participants (0b9b1a35) to_entity_id  the debate entity's
--                   → user, Claims (e614cce1) → what was debated                      created_at
--   follow          relations Following (f374b8f2) from a personal      from_entity_id  never decays
--                   space entity, in that same space, to a Topic
--   interested,     personalization.external_interest_signals, a hook   user_id       occurred_at
--   not_interested  for signals that live outside gaia (GEO-2862). Nothing writes it yet.
--
-- Measured against the public API on 2026-10-05: all 285 voters are Personal spaces; 756 of 786
-- Reply to relations are authored in a Personal space (the 30 in DAO spaces cannot be attributed
-- and are skipped); every Participants target and every Following source is a Personal space id.
--
-- Each activity credits the TOPICS of the thing it is about (the Topics relation, 806d52bc),
-- except a follow, which names its topic directly. A question debate, or Interested on a question,
-- credits the question's topics by the same route, whatever the debated entity's type.
--
-- ONE EVENT PER (user, kind, source). Voting up and agreeing on the same claim is one vote; ten
-- comments on one claim are one comment. That is what lets a card say "Because you voted on 4
-- claims about AI safety", and it stops one account running a topic up by repetition, which costs
-- nothing. Agree and disagree, up and down, all count the same: interest is caring, not side.
--
-- WEIGHTS AND DECAY ARE CONFIGURATION (interest_signal_weights, interest_config), not literals, and
-- weights are applied at read time: a stored row holds the kind's decayed COUNT, so changing a
-- weight takes effect on the next read. Changing the half-life takes effect at the next refit.
-- Decay is exact at read time: every event decays at the same rate, so a sum computed at time c
-- and decayed to now equals the sum computed now.
--
-- SPREADING. Activity on a narrow topic lifts the topics that most often co-appear with it on the
-- same entities (topic_cooccurrence, refreshed nightly, cosine similarity, top N per topic), by
-- spread_fraction × similarity × the topic's own weight, and a topic hands out at most
-- spread_fraction of its weight in all (similarities are scaled down when they sum past 1).
-- Similarity is at most 1 and the fraction below 1, so a neighbour always gets less than the topic
-- itself. Applied at read time, so it always uses the current co-occurrence table and decay.
--
-- FRESHNESS. `sweep_user_topic_interest` recomputes every user with activity since its last run
-- (less a lookback that absorbs indexing lag) from source, every couple of minutes, from
-- ranking-indexer's `topic_interest` CronJob. `refit_user_topic_interest` recomputes everyone
-- nightly and records how many rows disagreed with the incremental state — the things the sweep
-- cannot see: a Topics relation added to a claim people already voted on, a deleted comment or
-- follow, or indexing lag longer than the lookback. Both recompute a user from source by the same
-- function, so they can only disagree by what the sweep failed to notice.
--
-- ISOLATION. Nothing here is in the vote-indexer's write path: it only reads user_votes and the
-- graph, from its own CronJob, so a failure here cannot stop or delay vote indexing. It never
-- writes to the graph or creates a follow.
--
-- PRIVACY. Everything lives in its own schema, `personalization`, which PostGraphile does not
-- introspect (it is given ["public"] only), so none of it can be read through the public API.
-- hiddenSchemaSurface.test.ts asserts that. The serving path (GEO-3140) calls
-- `personalization.user_topic_weights` server-side.

CREATE SCHEMA IF NOT EXISTS "personalization";
--> statement-breakpoint

-- Single-row tunables. A singleton keyed on `true`, like entity_ranking_config.
CREATE TABLE IF NOT EXISTS "personalization"."interest_config" (
  "id" boolean PRIMARY KEY DEFAULT true CHECK ("id"),
  -- Activity this many days old counts half.
  "half_life_days" double precision NOT NULL DEFAULT 30 CHECK ("half_life_days" > 0),
  -- A neighbour gets at most this fraction of a topic's own weight. Must stay below 1, which is
  -- what keeps a related topic below the topic itself.
  "spread_fraction" double precision NOT NULL DEFAULT 0.2
    CHECK ("spread_fraction" >= 0 AND "spread_fraction" < 1),
  -- Neighbours kept per topic in topic_cooccurrence.
  "spread_max_neighbours" integer NOT NULL DEFAULT 10 CHECK ("spread_max_neighbours" >= 0),
  -- A pair must share at least this many entities to count as related.
  "cooccurrence_min_support" integer NOT NULL DEFAULT 3 CHECK ("cooccurrence_min_support" >= 1),
  -- Entities tagged with more topics than this are left out of co-occurrence: a catch-all tag
  -- list says little about which two topics belong together, and its pairs grow quadratically.
  "cooccurrence_max_topics_per_entity" integer NOT NULL DEFAULT 20
    CHECK ("cooccurrence_max_topics_per_entity" >= 2),
  -- How far behind its last run the sweep looks, to absorb the gap between an event's chain
  -- timestamp and when it is indexed. Anything later still reaches the nightly refit.
  "sweep_lookback" interval NOT NULL DEFAULT '1 hour' CHECK ("sweep_lookback" >= interval '0')
);
--> statement-breakpoint
INSERT INTO "personalization"."interest_config" ("id") VALUES (true) ON CONFLICT DO NOTHING;
--> statement-breakpoint

-- Starting weights from the spec ("Personalization from positions", Phase 1). Starting guesses, to
-- be tuned by the feed evaluation work: change a row, not code.
CREATE TABLE IF NOT EXISTS "personalization"."interest_signal_weights" (
  "kind" text PRIMARY KEY,
  "weight" double precision NOT NULL,
  -- false: the signal is a standing statement (a follow) and does not fade.
  "decays" boolean NOT NULL DEFAULT true,
  "note" text
);
--> statement-breakpoint
INSERT INTO "personalization"."interest_signal_weights" ("kind", "weight", "decays", "note") VALUES
  ('vote',           1.0, true,  'Any vote on an entity: up, down, agree, disagree, verify, dispute.'),
  ('comment',        1.5, true,  'A comment on the entity, or anywhere in its reply thread.'),
  ('debate',         2.0, true,  'Took part in a published debate about the entity.'),
  ('follow',         3.0, false, 'Follows the topic. Fixed: does not decay.'),
  ('interested',     2.0, true,  'Interested on a question. Needs a pipe from outside gaia.'),
  ('not_interested', -3.0, true, 'Not interested (GEO-2862). Needs a pipe from outside gaia.')
ON CONFLICT ("kind") DO NOTHING;
--> statement-breakpoint

-- The hook for signals gaia does not see today. Interested / Not interested live in geo-chat
-- (GEO-2862); whatever pipes them in writes rows here and they flow through everything below
-- with no other change. One row per (user, object, kind); re-recording moves occurred_at.
CREATE TABLE IF NOT EXISTS "personalization"."external_interest_signals" (
  "user_id" uuid NOT NULL,
  "object_id" uuid NOT NULL,
  "kind" text NOT NULL REFERENCES "personalization"."interest_signal_weights" ("kind"),
  "occurred_at" timestamptz NOT NULL,
  -- When it reached gaia, which is what the sweep keys on.
  "recorded_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("user_id", "object_id", "kind")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "external_interest_signals_recorded_at_idx"
  ON "personalization"."external_interest_signals" ("recorded_at");
--> statement-breakpoint

-- The learned state: per (user, topic, kind), the decayed count of distinct sources as of
-- computed_at. Weights are applied at read time. The kind with the largest contribution, its count
-- and its strongest source are the reason a card can give.
CREATE TABLE IF NOT EXISTS "personalization"."user_topic_signals" (
  "user_id" uuid NOT NULL,
  "topic_id" uuid NOT NULL,
  "kind" text NOT NULL,
  -- Σ 0.5^(age / half-life) over the kind's sources, ages measured at computed_at. For a kind
  -- that does not decay, the plain count.
  "decayed_count" double precision NOT NULL,
  "event_count" integer NOT NULL,
  "last_event_at" timestamptz,
  -- The source that contributed most: the claim voted on, the entity commented on, the debate,
  -- the topic followed.
  "top_object_id" uuid NOT NULL,
  "computed_at" timestamptz NOT NULL,
  PRIMARY KEY ("user_id", "topic_id", "kind")
);
--> statement-breakpoint

-- Related topics, for spreading. Directed rows (both directions are stored) so the read is one
-- index range per topic the user has.
CREATE TABLE IF NOT EXISTS "personalization"."topic_cooccurrence" (
  "topic_id" uuid NOT NULL,
  "neighbour_id" uuid NOT NULL,
  -- shared / sqrt(size_a × size_b): 1 when two topics always appear together, and a broad topic
  -- that co-appears with everything is not everyone's neighbour.
  "similarity" double precision NOT NULL CHECK ("similarity" > 0 AND "similarity" <= 1),
  "support" integer NOT NULL,
  PRIMARY KEY ("topic_id", "neighbour_id")
);
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "personalization"."interest_sweep_state" (
  "id" text PRIMARY KEY,
  "last_run_at" timestamptz NOT NULL
);
--> statement-breakpoint

-- One row per nightly refit: the agreement check between the incremental state and a from-scratch
-- recompute. A rising disagreement count means the sweep is missing something.
CREATE TABLE IF NOT EXISTS "personalization"."interest_refit_runs" (
  "ran_at" timestamptz PRIMARY KEY,
  "users" integer NOT NULL,
  "signal_rows" integer NOT NULL,
  "disagreeing_rows" integer NOT NULL,
  "max_abs_diff" double precision NOT NULL,
  "cooccurrence_rows" integer
);
--> statement-breakpoint

-- entities.created_at / updated_at are text holding epoch seconds. NULL for anything else.
CREATE OR REPLACE FUNCTION personalization.epoch_text_ts(t text)
RETURNS timestamptz
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT CASE WHEN t ~ '^[0-9]{1,12}$' AND t::bigint > 0 THEN to_timestamp(t::bigint) END
$$;
--> statement-breakpoint

-- Every activity event for the given users, one row per (user, kind, source, object).
-- `object_is_topic` says the object IS the topic (a follow) rather than something tagged with
-- topics. Takes the user list explicitly rather than NULL-for-all: an `IS NULL OR` guard defeats
-- the index lookups below.
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
  )
  -- Votes: any kind, either direction, on an entity. vote_type 2 is a removal.
  SELECT v.user_id, 'vote'::text, v.object_id, v.object_id, false, max(v.voted_at)
  FROM public.user_votes v
  WHERE v.user_id = ANY(user_ids) AND v.object_type = 0 AND v.vote_type IN (0, 1)
    AND EXISTS (SELECT 1 FROM users u WHERE u.id = v.user_id)
  GROUP BY v.user_id, v.object_id

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
  -- Follows of a Topic, stated by the user in their own space. A relation in someone else's space
  -- saying this user follows something is not the user's statement.
  SELECT DISTINCT f.from_entity_id, 'follow', f.to_entity_id, f.to_entity_id, true,
         personalization.epoch_text_ts(re.created_at)
  FROM public.relations f
  JOIN users u ON u.id = f.from_entity_id
  LEFT JOIN public.entities re ON re.id = f.id
  WHERE f.type_id = 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid
    AND f.space_id = f.from_entity_id
    AND EXISTS (SELECT 1 FROM public.relations ty
                WHERE ty.from_entity_id = f.to_entity_id
                  AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                  AND ty.to_entity_id = '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid)

  UNION ALL
  SELECT x.user_id, x.kind, x.object_id, x.object_id, false, x.occurred_at
  FROM personalization.external_interest_signals x
  WHERE x.user_id = ANY(user_ids)
    AND EXISTS (SELECT 1 FROM users u WHERE u.id = x.user_id)
$$;
--> statement-breakpoint

-- Per (user, topic, kind) as of `as_of`: the decayed count of distinct sources, the raw count, the
-- latest event and the strongest source. The one computation both the sweep and the refit use.
CREATE OR REPLACE FUNCTION personalization.compute_user_topic_signals(user_ids uuid[], as_of timestamptz)
RETURNS TABLE (user_id uuid, topic_id uuid, kind text, decayed_count double precision,
               event_count integer, last_event_at timestamptz, top_object_id uuid)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (SELECT half_life_days * 86400.0 AS half_life_s FROM personalization.interest_config),
  ev AS (
    SELECT e.*, w.decays
    FROM personalization.user_interest_events(user_ids) e
    JOIN personalization.interest_signal_weights w ON w.kind = e.kind
    -- A decaying event needs a time to decay from; a follow does not.
    WHERE e.occurred_at IS NOT NULL OR NOT w.decays
  ),
  credited AS (
    SELECT ev.user_id, t.topic_id, ev.kind, ev.source_id, ev.decays, max(ev.occurred_at) AS occurred_at
    FROM ev
    CROSS JOIN LATERAL (
      SELECT ev.object_id AS topic_id WHERE ev.object_is_topic
      UNION
      SELECT r.to_entity_id FROM public.relations r
      WHERE NOT ev.object_is_topic
        AND r.from_entity_id = ev.object_id
        AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
    ) t
    -- One credit per (user, topic, kind, source): a debate over two claims that share a topic
    -- counts once for it.
    GROUP BY ev.user_id, t.topic_id, ev.kind, ev.source_id, ev.decays
  ),
  weighted AS (
    SELECT c.*,
           CASE WHEN c.decays
                THEN power(0.5, GREATEST(0, extract(epoch FROM as_of - c.occurred_at)) / cfg.half_life_s)
                ELSE 1 END AS factor
    FROM credited c CROSS JOIN cfg
  )
  SELECT w.user_id, w.topic_id, w.kind, sum(w.factor)::double precision, count(*)::integer,
         max(w.occurred_at), (array_agg(w.source_id ORDER BY w.factor DESC, w.source_id))[1]
  FROM weighted w
  GROUP BY w.user_id, w.topic_id, w.kind
$$;
--> statement-breakpoint

-- Replace the stored state for these users with a recompute from source. A user with no activity
-- ends with no rows. Returns the rows written.
CREATE OR REPLACE FUNCTION personalization.recompute_user_topic_interest(user_ids uuid[], as_of timestamptz)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE written integer;
BEGIN
  DELETE FROM personalization.user_topic_signals s WHERE s.user_id = ANY(user_ids);
  INSERT INTO personalization.user_topic_signals
    (user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id, computed_at)
  SELECT c.user_id, c.topic_id, c.kind, c.decayed_count, c.event_count, c.last_event_at,
         c.top_object_id, as_of
  FROM personalization.compute_user_topic_signals(user_ids, as_of) c;
  GET DIAGNOSTICS written = ROW_COUNT;
  RETURN written;
END;
$$;
--> statement-breakpoint

-- Users with anything new since `since`, by event time. Comments, debates and follows are found
-- through the entity their relation hangs from: kg-indexer bumps a relation's FROM entity's
-- updated_at when the relation is created. A deleted relation, or a Topics relation added to
-- something already engaged with, does not show up here; the nightly refit covers those.
CREATE OR REPLACE FUNCTION personalization.dirty_interest_users(since timestamptz)
RETURNS uuid[]
LANGUAGE plpgsql STABLE AS $$
DECLARE
  since_epoch bigint := GREATEST(0, floor(extract(epoch FROM since)))::bigint;
  -- entities.updated_at is epoch-seconds TEXT; a text comparison orders correctly only between
  -- strings of equal length, and live values are all 10 digits. Anything before 2001 means "all".
  since_text text := CASE WHEN since_epoch < 1000000000 THEN '0' ELSE since_epoch::text END;
  result uuid[];
BEGIN
  SELECT coalesce(array_agg(DISTINCT u.user_id), '{}') INTO result
  FROM (
    SELECT v.user_id FROM public.user_votes v WHERE v.voted_at >= since
    UNION ALL
    SELECT CASE r.type_id
             WHEN '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid THEN r.space_id       -- comment author
             WHEN '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid THEN r.to_entity_id   -- debater
             ELSE r.from_entity_id                                                   -- follower
           END
    FROM public.entities e
    JOIN public.relations r ON r.from_entity_id = e.id
    WHERE e.updated_at >= since_text
      AND r.type_id IN ('310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid,
                        '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid,
                        'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid)
    UNION ALL
    SELECT x.user_id FROM personalization.external_interest_signals x WHERE x.recorded_at >= since
  ) u
  WHERE EXISTS (SELECT 1 FROM public.spaces s WHERE s.id = u.user_id AND s.type = 'Personal');
  RETURN result;
END;
$$;
--> statement-breakpoint

-- The incremental pass, every couple of minutes. Recomputes everyone with activity since the last
-- run less the lookback, then moves the cursor, all in the caller's transaction: a failed run
-- leaves the cursor where it was and the next one repeats it, and a repeated run rewrites the same
-- rows. Returns NULL dirty_users when the nightly refit holds the lock (that run is skipped).
CREATE OR REPLACE FUNCTION personalization.sweep_user_topic_interest(as_of timestamptz DEFAULT now())
RETURNS TABLE (dirty_users integer, signal_rows integer, since timestamptz)
LANGUAGE plpgsql AS $$
DECLARE
  last_run timestamptz;
  lookback interval;
  cutoff timestamptz;
  users uuid[];
  written integer;
BEGIN
  IF NOT pg_try_advisory_xact_lock(hashtext('personalization.user_topic_interest')) THEN
    RETURN QUERY SELECT NULL::integer, 0, NULL::timestamptz;
    RETURN;
  END IF;

  SELECT s.last_run_at INTO last_run
  FROM personalization.interest_sweep_state s WHERE s.id = 'incremental' FOR UPDATE;
  SELECT c.sweep_lookback INTO lookback FROM personalization.interest_config c;
  IF last_run IS NULL THEN
    -- First run: every person, which makes it the backfill. Not "activity since the epoch": that
    -- would find people only through the timestamps the incremental path relies on.
    cutoff := 'epoch'::timestamptz;
    SELECT coalesce(array_agg(s.id), '{}') INTO users FROM public.spaces s WHERE s.type = 'Personal';
  ELSE
    cutoff := last_run - lookback;
    users := personalization.dirty_interest_users(cutoff);
  END IF;
  written := personalization.recompute_user_topic_interest(users, as_of);

  INSERT INTO personalization.interest_sweep_state (id, last_run_at) VALUES ('incremental', as_of)
  ON CONFLICT (id) DO UPDATE SET last_run_at = GREATEST(interest_sweep_state.last_run_at, EXCLUDED.last_run_at);

  RETURN QUERY SELECT cardinality(users), written, cutoff;
END;
$$;
--> statement-breakpoint

-- Rebuilds topic_cooccurrence from the For you topic ranking (0096), which already holds only the
-- (topic, entity) pairs that can be shown: named, not System, not excluded. Returns rows written.
CREATE OR REPLACE FUNCTION personalization.refresh_topic_cooccurrence()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  cfg personalization.interest_config;
  written integer;
BEGIN
  SELECT * INTO cfg FROM personalization.interest_config;

  CREATE TEMP TABLE IF NOT EXISTS cooc_tags (topic_id uuid, entity_id uuid) ON COMMIT DROP;
  TRUNCATE cooc_tags;
  INSERT INTO cooc_tags
  SELECT DISTINCT etr.topic_id, etr.entity_id FROM public.entity_topic_ranking etr;
  DELETE FROM cooc_tags t
  WHERE t.entity_id IN (SELECT entity_id FROM cooc_tags GROUP BY entity_id
                        HAVING count(*) > cfg.cooccurrence_max_topics_per_entity);
  ANALYZE cooc_tags;

  DELETE FROM personalization.topic_cooccurrence;
  INSERT INTO personalization.topic_cooccurrence (topic_id, neighbour_id, similarity, support)
  WITH sizes AS (SELECT topic_id, count(*) AS n FROM cooc_tags GROUP BY topic_id),
  pairs AS (
    SELECT a.topic_id AS a, b.topic_id AS b, count(*) AS shared
    FROM cooc_tags a JOIN cooc_tags b ON b.entity_id = a.entity_id AND b.topic_id <> a.topic_id
    GROUP BY a.topic_id, b.topic_id
    HAVING count(*) >= cfg.cooccurrence_min_support
  ),
  scored AS (
    SELECT p.a, p.b, p.shared,
           LEAST(1.0, p.shared / sqrt(sa.n::double precision * sb.n::double precision)) AS sim
    FROM pairs p JOIN sizes sa ON sa.topic_id = p.a JOIN sizes sb ON sb.topic_id = p.b
  ),
  ranked AS (
    SELECT s.*, row_number() OVER (PARTITION BY s.a ORDER BY s.sim DESC, s.b) AS rn FROM scored s
  )
  SELECT a, b, sim, shared::integer FROM ranked WHERE rn <= cfg.spread_max_neighbours;
  GET DIAGNOSTICS written = ROW_COUNT;
  RETURN written;
END;
$$;
--> statement-breakpoint

-- The nightly refit: everyone from scratch, compared with what the sweep left, then swapped in.
-- Records the comparison in interest_refit_runs and returns it.
CREATE OR REPLACE FUNCTION personalization.refit_user_topic_interest(
  as_of timestamptz DEFAULT now(),
  cooccurrence_rows integer DEFAULT NULL
)
RETURNS TABLE (users integer, signal_rows integer, disagreeing_rows integer, max_abs_diff double precision)
LANGUAGE plpgsql AS $$
DECLARE
  all_users uuid[];
  half_life_s double precision;
  n_rows integer;
  n_users integer;
  n_disagree integer;
  max_diff double precision;
BEGIN
  -- Waits for a running sweep rather than skipping: the refit is the one that must happen.
  PERFORM pg_advisory_xact_lock(hashtext('personalization.user_topic_interest'));
  SELECT c.half_life_days * 86400.0 INTO half_life_s FROM personalization.interest_config c;

  SELECT coalesce(array_agg(s.id), '{}') INTO all_users FROM public.spaces s WHERE s.type = 'Personal';

  CREATE TEMP TABLE IF NOT EXISTS refit_signals (
    user_id uuid, topic_id uuid, kind text, decayed_count double precision, event_count integer,
    last_event_at timestamptz, top_object_id uuid
  ) ON COMMIT DROP;
  TRUNCATE refit_signals;
  INSERT INTO refit_signals SELECT * FROM personalization.compute_user_topic_signals(all_users, as_of);

  -- Compare in the same units: the stored count decayed from its computed_at to as_of.
  SELECT count(*) FILTER (WHERE d.diff > 1e-9 * GREATEST(1, d.fresh) OR d.count_differs),
         coalesce(max(d.diff), 0)
    INTO n_disagree, max_diff
  FROM (
    SELECT abs(coalesce(f.decayed_count, 0) - coalesce(o.now_count, 0)) AS diff,
           coalesce(f.decayed_count, 0) AS fresh,
           f.event_count IS DISTINCT FROM o.event_count AS count_differs
    FROM refit_signals f
    FULL JOIN (
      SELECT s.user_id, s.topic_id, s.kind, s.event_count,
             s.decayed_count * CASE WHEN w.decays IS FALSE THEN 1
               ELSE power(0.5, GREATEST(0, extract(epoch FROM as_of - s.computed_at)) / half_life_s) END
               AS now_count
      FROM personalization.user_topic_signals s
      LEFT JOIN personalization.interest_signal_weights w ON w.kind = s.kind
    ) o USING (user_id, topic_id, kind)
  ) d;

  DELETE FROM personalization.user_topic_signals;
  INSERT INTO personalization.user_topic_signals
    (user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id, computed_at)
  SELECT r.user_id, r.topic_id, r.kind, r.decayed_count, r.event_count, r.last_event_at, r.top_object_id, as_of
  FROM refit_signals r;
  GET DIAGNOSTICS n_rows = ROW_COUNT;
  SELECT count(DISTINCT r.user_id) INTO n_users FROM refit_signals r;

  INSERT INTO personalization.interest_refit_runs
    (ran_at, users, signal_rows, disagreeing_rows, max_abs_diff, cooccurrence_rows)
  VALUES (as_of, n_users, n_rows, n_disagree, max_diff, refit_user_topic_interest.cooccurrence_rows)
  ON CONFLICT (ran_at) DO UPDATE SET users = EXCLUDED.users, signal_rows = EXCLUDED.signal_rows,
    disagreeing_rows = EXCLUDED.disagreeing_rows, max_abs_diff = EXCLUDED.max_abs_diff,
    cooccurrence_rows = EXCLUDED.cooccurrence_rows;

  RETURN QUERY SELECT n_users, n_rows, n_disagree, max_diff;
END;
$$;
--> statement-breakpoint

-- Every contribution to one user's topics as of `as_of`, weighted and decayed: one row per own
-- (topic, kind), and one 'related' row per (topic, source topic) from spreading. The basis of
-- both reads below.
CREATE OR REPLACE FUNCTION personalization.user_topic_contributions(p_user_id uuid, as_of timestamptz DEFAULT now())
RETURNS TABLE (topic_id uuid, kind text, contribution double precision, event_count integer,
               last_event_at timestamptz, top_object_id uuid, via_topic_id uuid)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (SELECT * FROM personalization.interest_config),
  own_kinds AS (
    SELECT s.topic_id, s.kind,
           w.weight * s.decayed_count
             * CASE WHEN w.decays
                    THEN power(0.5, GREATEST(0, extract(epoch FROM as_of - s.computed_at)) / (cfg.half_life_days * 86400.0))
                    ELSE 1 END AS contribution,
           s.event_count, s.last_event_at, s.top_object_id
    FROM personalization.user_topic_signals s
    JOIN personalization.interest_signal_weights w ON w.kind = s.kind
    CROSS JOIN cfg
    WHERE s.user_id = p_user_id
  ),
  own AS (SELECT o.topic_id, sum(o.contribution) AS weight FROM own_kinds o GROUP BY o.topic_id),
  related AS (
    -- Only positive interest spreads: Not interested in one topic says nothing about its
    -- neighbours. A topic hands out at most spread_fraction of its weight in total: when its
    -- neighbours' similarities sum past 1 they share that budget in proportion. Without the cap a
    -- topic with ten close neighbours gave away twice its own weight, and on real data a heavy
    -- user's unengaged neighbours came out level with what they actually voted on.
    SELECT c.neighbour_id AS topic_id,
           cfg.spread_fraction * o.weight * c.similarity
             / GREATEST(1.0, sum(c.similarity) OVER (PARTITION BY o.topic_id)) AS contribution,
           o.topic_id AS via_topic_id
    FROM own o
    JOIN personalization.topic_cooccurrence c ON c.topic_id = o.topic_id
    CROSS JOIN cfg
    WHERE o.weight > 0 AND cfg.spread_fraction > 0
  )
  SELECT o.topic_id, o.kind, o.contribution, o.event_count, o.last_event_at, o.top_object_id, NULL::uuid
  FROM own_kinds o
  UNION ALL
  SELECT r.topic_id, 'related', r.contribution, NULL, NULL, NULL, r.via_topic_id
  FROM related r
$$;
--> statement-breakpoint

-- THE READ for For you (GEO-3140 calls this server-side): a user's top topics by weight, with the
-- reason a card can give. `top_kind` is the single largest contributor: a signal kind with its
-- count and strongest source (e.g. vote, 4, <claim>), or 'related' with the topic it spread from.
-- Only topics with positive weight; a user with no activity and no follows gets no rows, and
-- therefore plain Best.
CREATE OR REPLACE FUNCTION personalization.user_topic_weights(
  p_user_id uuid,
  max_topics integer DEFAULT 50,
  as_of timestamptz DEFAULT now()
)
RETURNS TABLE (topic_id uuid, weight double precision, own_weight double precision,
               related_weight double precision, top_kind text, top_kind_count integer,
               top_object_id uuid, related_via_topic_id uuid)
LANGUAGE sql STABLE AS $$
  WITH c AS (SELECT * FROM personalization.user_topic_contributions(p_user_id, as_of)),
  -- A topic reached through several neighbours: the one that gave it the most.
  rel AS (
    SELECT c.topic_id, sum(c.contribution) AS related_weight,
           (array_agg(c.via_topic_id ORDER BY c.contribution DESC, c.via_topic_id))[1] AS via
    FROM c WHERE c.kind = 'related' GROUP BY c.topic_id
  ),
  own AS (SELECT c.topic_id, sum(c.contribution) AS own_weight FROM c WHERE c.kind <> 'related' GROUP BY c.topic_id),
  candidates AS (
    SELECT c.topic_id, c.kind, c.contribution, c.event_count, c.top_object_id
    FROM c WHERE c.kind <> 'related'
    UNION ALL
    SELECT rel.topic_id, 'related', rel.related_weight, NULL, rel.via FROM rel
  ),
  top AS (
    SELECT DISTINCT ON (cd.topic_id) cd.topic_id, cd.kind, cd.event_count, cd.top_object_id
    FROM candidates cd ORDER BY cd.topic_id, cd.contribution DESC, cd.kind
  )
  SELECT t.topic_id,
         coalesce(own.own_weight, 0) + coalesce(rel.related_weight, 0),
         coalesce(own.own_weight, 0), coalesce(rel.related_weight, 0),
         t.kind, t.event_count, t.top_object_id, rel.via
  FROM top t
  LEFT JOIN own ON own.topic_id = t.topic_id
  LEFT JOIN rel ON rel.topic_id = t.topic_id
  WHERE coalesce(own.own_weight, 0) + coalesce(rel.related_weight, 0) > 0
  ORDER BY 2 DESC, t.topic_id
  LIMIT max_topics
$$;
--> statement-breakpoint

-- Why a topic is weighted for a user, every contribution largest first: "Because you voted on 4
-- claims about AI safety" is the first row's (kind, event_count) and the topic's name.
CREATE OR REPLACE FUNCTION personalization.user_topic_interest_reasons(
  p_user_id uuid,
  p_topic_id uuid,
  as_of timestamptz DEFAULT now()
)
RETURNS TABLE (kind text, contribution double precision, event_count integer,
               last_event_at timestamptz, top_object_id uuid, via_topic_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT c.kind, c.contribution, c.event_count, c.last_event_at, c.top_object_id, c.via_topic_id
  FROM personalization.user_topic_contributions(p_user_id, as_of) c
  WHERE c.topic_id = p_topic_id
  ORDER BY c.contribution DESC, c.kind, c.via_topic_id
$$;
--> statement-breakpoint

-- If the app connects as a separate role from the migrator, it needs the new schema too. Guarded,
-- because environments that use one role for both have no such role.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA personalization TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA personalization TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
