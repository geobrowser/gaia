-- Assertions for 0078: stance votes (positions) feed the ranking score.
--
-- Run per drizzle/tests/README.md. Truncates its own fixtures so it passes in
-- any order relative to the other suites.
--
-- The central risk this pins is NOT "does the term compute" — it is that the term
-- is too small to change an ordering, which is how a capped `intrinsic`-sized
-- bonus would have failed: every arithmetic assertion green, feed unchanged. So
-- the load-bearing assertions here compare ORDERINGS across a realistic age gap,
-- not just values.

\set ON_ERROR_STOP on

-- Same signature as the other suites (parameter names included — CREATE OR
-- REPLACE cannot rename an input parameter, so a divergent name fails outright).
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions;

-- entity_ranking_config is a single fixed row, so TRUNCATE cannot reset it and a
-- suite that fails midway leaves its armed values behind — which then breaks the
-- NEXT run at a completely unrelated assertion. Reset it explicitly.
UPDATE entity_ranking_config
   SET participation_weight = 0, participation_cap = 10, tau_seconds = 45000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Pure function behaviour
-- ---------------------------------------------------------------------------
SELECT assert(public.entity_participation_score(0, 7, 10) = 0,
  'no positions yields exactly 0, not -Infinity (ln(1+0), never ln(0))');
SELECT assert(public.entity_participation_score(50, 0, 10) = 0,
  'weight 0 is inert whatever the position count — the shipped default must not move any score');
SELECT assert(public.entity_participation_score(5, 7, 10) = 10,
  'the cap binds: 7*ln(6) = 12.5 clamps to 10');
SELECT assert(public.entity_participation_score(5, 2, 100)
            > public.entity_participation_score(1, 2, 100),
  'more positions scores higher');
SELECT assert(public.entity_participation_score(2, 2, 100) - public.entity_participation_score(1, 2, 100)
            > public.entity_participation_score(11, 2, 100) - public.entity_participation_score(10, 2, 100),
  'diminishing returns — the first position is worth more than the eleventh');
SELECT assert(public.entity_participation_score(-5, 2, 100) = 0,
  'a negative count cannot produce a negative term (would silently demote)');
SELECT assert(public.entity_participation_score(NULL, NULL, NULL) = 0,
  'NULL inputs yield 0 rather than NULL, which would null the whole ranking score');

-- ---------------------------------------------------------------------------
-- Fixtures. Ages chosen to mirror the reported pair: 13.55 days apart.
--   OLD_WITH   = 1785478598, 5 positions   (the claim that ranked too low)
--   NEW_WITHOUT= 1786649398, 0 positions   (the claim that outranked it)
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at) VALUES
  ('a0000000-0000-0000-0000-000000000001','1785478598'),  -- old, 5 positions
  ('a0000000-0000-0000-0000-000000000002','1786649398'),  -- new, 0 positions
  ('a0000000-0000-0000-0000-000000000003','1786649398'),  -- new, 5 positions
  ('a0000000-0000-0000-0000-000000000004','1786649398');  -- new, veracity only

-- 5 positions split 3 agree / 2 disagree on the old claim, plus the single
-- curation downvote the reported entity actually carried.
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('a0000000-0000-0000-0000-000000000001',0,'bbbbbbbb-0000-0000-0000-000000000000',1,3,2),
  ('a0000000-0000-0000-0000-000000000001',0,'bbbbbbbb-0000-0000-0000-000000000000',0,0,1),
  ('a0000000-0000-0000-0000-000000000003',0,'bbbbbbbb-0000-0000-0000-000000000000',1,5,0),
  ('a0000000-0000-0000-0000-000000000004',0,'bbbbbbbb-0000-0000-0000-000000000000',2,900,0);

-- ---------------------------------------------------------------------------
-- Inert by default. This is the guarantee that applying the migration to
-- production changes nothing until someone deliberately arms it.
-- ---------------------------------------------------------------------------
-- Read from the catalog, NOT from the row: this suite resets the row above, so a
-- row-value assertion would only be checking its own UPDATE. The column DEFAULT
-- is the actual shipped guarantee — that applying 0078 to production arms
-- nothing.
SELECT assert((SELECT column_default::numeric FROM information_schema.columns
                WHERE table_name = 'entity_ranking_config'
                  AND column_name = 'participation_weight') = 0,
  'participation_weight column DEFAULT is 0 — applying this migration must not arm the term');
SELECT assert((SELECT column_default::numeric FROM information_schema.columns
                WHERE table_name = 'entity_ranking_scores'
                  AND column_name = 'participation_score') = 0,
  'participation_score column DEFAULT is 0 for rows written before a rescore');

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  'a0000000-0000-0000-0000-000000000001','a0000000-0000-0000-0000-000000000002',
  'a0000000-0000-0000-0000-000000000003','a0000000-0000-0000-0000-000000000004']::uuid[]) = 4,
  'refresh scored all 4 entities');

SELECT assert((SELECT count(*) FROM entity_ranking_scores WHERE participation_score <> 0) = 0,
  'at weight 0 no entity gets a participation term');
SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000002')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001'),
  'BASELINE: at weight 0 the newer position-less claim still outranks the older one with 5 positions (the reported bug, reproduced)');

-- Stance counts are recorded even while the term is inert, so the feed can be
-- explained before it is retuned.
SELECT assert((SELECT stance_positive FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001') = 3
          AND (SELECT stance_negative FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001') = 2,
  'agrees/disagrees are stored separately even at weight 0');
SELECT assert((SELECT positive FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001') = 0,
  'stance rows must NOT leak into the curation tally');
SELECT assert((SELECT quality_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000003')
            = (SELECT quality_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000002'),
  'positions do not move the Wilson quality term — a position is engagement, not approval');
SELECT assert((SELECT participation_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000004') = 0,
  'veracity votes contribute no participation');

-- ---------------------------------------------------------------------------
-- Armed.
--
-- tau is set explicitly to 100000 rather than left at the migration default of
-- 45000, because that is the value the LIVE v2 database runs (derived from two
-- production entities' stored scores: a 13.55-day age gap accounts for 11.68
-- score units, which only solves at tau ~= 1e5; at the 45000 default the same
-- pair would be 26 units apart). Leaving it at the default made the central
-- assertion below fail, which is the point worth recording: the weight required
-- is a function of tau, so the two knobs cannot be tuned independently. Anyone
-- changing tau in production must re-derive the weight.
--
-- Weight 7 then flips the reported pair with ~0.4 units to spare:
--   participation = 7*ln(1+5)          = 12.54
--   to overcome   = 11.71 (age) + 0.43 (the single curation downvote) = 12.14
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 100, tau_seconds = 100000
 WHERE id;

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  'a0000000-0000-0000-0000-000000000001','a0000000-0000-0000-0000-000000000002',
  'a0000000-0000-0000-0000-000000000003','a0000000-0000-0000-0000-000000000004']::uuid[]) = 4,
  'rescore after arming');

-- THE assertion this migration exists for.
SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000002'),
  'armed: the 13.55-day-older claim with 5 positions now outranks the newer one with none');

SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000003')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000002'),
  'armed: among same-age claims, the one with positions outranks the one without');

SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000003')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001'),
  'armed: equal positions, newer wins — the recency term survives, it is not swamped');

SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000004')
            = (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000002'),
  'armed: a veracity landslide changes nothing — 900 verifications must not buy rank');

-- Direction-agnostic: 5 positions is 5 positions however they split. Pinning this
-- is what stops someone "improving" the term into agrees - disagrees, which would
-- bury contested claims.
SELECT assert((SELECT participation_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000001')
            = (SELECT participation_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000003'),
  'armed: 3-agree/2-disagree scores the same participation as 5-agree/0-disagree');

-- ---------------------------------------------------------------------------
-- The cap is a manipulation guardrail, so it has to actually bind.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET participation_cap = 1 WHERE id;
SELECT assert(refresh_entity_ranking_scores(ARRAY['a0000000-0000-0000-0000-000000000003']::uuid[]) = 1,
  'rescore under a tight cap');
SELECT assert((SELECT participation_score FROM entity_ranking_scores WHERE entity_id='a0000000-0000-0000-0000-000000000003') = 1,
  'a brigade cannot buy more than participation_cap units of rank');

-- Restore the shipped defaults so this suite leaves no armed config behind for
-- another suite to trip over. tau included — 0073's assertions are written
-- against the 45000 default.
UPDATE entity_ranking_config
   SET participation_weight = 0, participation_cap = 10, tau_seconds = 45000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Node-level exposure. The reporter's first move was to query rankingScore +
-- qualityScore + upvotes + downvotes to explain an ordering; positions have to be
-- reachable the same way or the next surprising rank is undebuggable from the API.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 100, tau_seconds = 100000
 WHERE id;
SELECT assert(refresh_entity_ranking_scores(ARRAY['a0000000-0000-0000-0000-000000000001']::uuid[]) = 1,
  'rescore before checking the computed columns');

SELECT assert((SELECT public.entities_agrees(e) FROM entities e
                WHERE e.id = 'a0000000-0000-0000-0000-000000000001') = 3,
  'entities_agrees exposes stance_positive (GraphQL: Entity.agrees)');
SELECT assert((SELECT public.entities_disagrees(e) FROM entities e
                WHERE e.id = 'a0000000-0000-0000-0000-000000000001') = 2,
  'entities_disagrees exposes stance_negative (GraphQL: Entity.disagrees)');
SELECT assert((SELECT public.entities_participation_score(e) FROM entities e
                WHERE e.id = 'a0000000-0000-0000-0000-000000000001') > 0,
  'entities_participation_score exposes the term so a rank can be explained without recomputing');

UPDATE entity_ranking_config
   SET participation_weight = 0, participation_cap = 10, tau_seconds = 45000
 WHERE id;

SELECT '0078 OK' AS result;
