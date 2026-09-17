-- Assertions for 0089: participation can no longer buy weeks of recency.
--
-- Run per drizzle/tests/README.md. Removes only its own fixtures, so it passes in any
-- order and is safe against a database that holds real rows.
--
-- The risk this is written against is the one 0078, 0079 and 0083 all name: every value
-- assertion passes and the feed does not change. A retune is even more prone to it than a
-- new term, because nothing errors when a parameter is merely the wrong size. So the
-- central assertions compare an ORDERING across a real recency gap, and each one is
-- constructed so that it FLIPS between the old parameters and the new. A test that passes
-- under both proves nothing about this change.
--
-- The four claims:
--   1. A well-voted older entity no longer outranks a newer unvoted one at ~10 days.
--      Under w7 it did (15.38 > 8.64); under w2.5 it does not (5.49 < 8.64).
--   2. Votes still decide between entities of the SAME age — the fix must not turn the
--      feed chronological, which is the failure mode of overcorrecting.
--   3. Participation still beats recency over a SHORT gap, so engagement is not inert.
--   4. The cap now binds where it is meant to (~120 votes), not effectively never.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

-- Deletes only its own rows rather than TRUNCATEing the ranking tables, which is what
-- 0078/0079/0083/0084 do. Those lists no longer work: `spaces` and `subspace_topics`
-- gained foreign keys to `entities` after they were written, and the closure now pulls
-- in `proposals`, `proposal_votes`, `subspaces` and `space_voting_settings` as well.
-- Truncating all of that to compare two scores is the wrong trade — and scoping the
-- cleanup by id means this file is safe to run against a database with real data in it.
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000089-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000089-%';
DELETE FROM entities              WHERE id::text        LIKE '00000089-%';

-- The values 0089 writes, SET explicitly rather than read back from the table.
--
-- Asserting that the config row still holds them would make this file order-dependent:
-- `entity_ranking_config` is a single shared row and 0078, 0079, 0083 and 0084 all
-- mutate it for their own fixtures, several without restoring it. A file that asserted
-- the migrated values would pass alone and fail in a suite, which is worse than not
-- checking. The migration itself is three lines and reviewable; what needs a test is
-- what these numbers do to an ORDERING, which is everything below.
UPDATE entity_ranking_config
   SET participation_weight = 2.5, participation_cap = 12,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. One day of age = tau/86400 = 0.864 score units.
--
--   OLD_VOTED  10 days older, 8 curation votes  -> participation 2.5*ln(9) = 5.49
--   NEW_QUIET  newest, no votes                 -> age advantage 10 * 0.864 = 8.64
--
-- Under the previous weight of 7 the participation term was 15.38 and OLD_VOTED won.
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES
  ('00000089-0000-4000-8000-000000000001', (extract(epoch from now())::bigint - 864000)::text, '0',
                                            (extract(epoch from now())::bigint - 864000)::text, '0'),
  ('00000089-0000-4000-8000-000000000002', extract(epoch from now())::bigint::text, '0',
                                            extract(epoch from now())::bigint::text, '0'),
  -- Same creation time as each other, differing only in votes (claim 2).
  ('00000089-0000-4000-8000-000000000003', extract(epoch from now())::bigint::text, '0',
                                            extract(epoch from now())::bigint::text, '0'),
  -- Two days older than NEW_QUIET, with votes (claim 3): 2 * 0.864 = 1.73 against
  -- 2.5*ln(5) = 4.02, so participation still wins over a short gap.
  ('00000089-0000-4000-8000-000000000004', (extract(epoch from now())::bigint - 172800)::text, '0',
                                            (extract(epoch from now())::bigint - 172800)::text, '0');

-- vote_kind 0 is curation, which 0083 made count toward participation alongside stance.
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('00000089-0000-4000-8000-000000000001', 0, '00000089-0000-4000-8000-0000000000aa', 0, 8, 0),
  ('00000089-0000-4000-8000-000000000003', 0, '00000089-0000-4000-8000-0000000000aa', 0, 8, 0),
  ('00000089-0000-4000-8000-000000000004', 0, '00000089-0000-4000-8000-0000000000aa', 0, 4, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY[
  '00000089-0000-4000-8000-000000000001',
  '00000089-0000-4000-8000-000000000002',
  '00000089-0000-4000-8000-000000000003',
  '00000089-0000-4000-8000-000000000004'
]::uuid[]);

-- 1. The entrenchment case: 8 votes no longer outrank ten days of recency.
DO $$
DECLARE old_score numeric; new_score numeric;
BEGIN
  SELECT ranking_score INTO old_score FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000001';
  SELECT ranking_score INTO new_score FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000002';
  PERFORM assert(new_score > old_score,
    format('a 10-day-newer unvoted entity outranks an 8-vote older one (new %s vs old %s) — '
           'this is the assertion that flips: under participation_weight 7 the older one won',
           new_score, old_score));
END $$;

-- 2. Votes still decide at equal age. Overcorrecting would make the feed chronological.
DO $$
DECLARE voted numeric; quiet numeric;
BEGIN
  SELECT ranking_score INTO voted FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000003';
  SELECT ranking_score INTO quiet FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000002';
  PERFORM assert(voted > quiet,
    format('at identical age, 8 votes still outrank 0 (%s vs %s)', voted, quiet));
END $$;

-- 3. Participation still beats recency over a short gap — the term is not inert.
DO $$
DECLARE older_voted numeric; newest numeric;
BEGIN
  SELECT ranking_score INTO older_voted FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000004';
  SELECT ranking_score INTO newest FROM entity_ranking_scores
   WHERE entity_id = '00000089-0000-4000-8000-000000000002';
  PERFORM assert(older_voted > newest,
    format('4 votes still outrank 2 days of recency (%s vs %s) — if this fails the retune '
           'went too far and engagement no longer counts', older_voted, newest));
END $$;

-- 4. The cap binds where it is meant to. At weight 2.5 a cap of 30 would need e^12
--    votes to bind, i.e. never; 12 binds at ~120.
DO $$
DECLARE at_119 numeric; at_200 numeric; capped numeric;
BEGIN
  SELECT participation_cap INTO capped FROM entity_ranking_config WHERE id;
  at_119 := public.entity_participation_score(119, 2.5, capped);
  at_200 := public.entity_participation_score(200, 2.5, capped);
  PERFORM assert(at_119 < capped,
    format('119 votes is below the cap (%s < %s)', at_119, capped));
  PERFORM assert(at_200 = capped,
    format('200 votes reaches the cap (%s = %s)', at_200, capped));
  PERFORM assert(public.entity_participation_score(19, 2.5, capped) < capped,
    'the highest vote count observed in a live feed (19) is nowhere near the cap — '
    'which is why lowering the CAP was not the fix');
END $$;

DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000089-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000089-%';
DELETE FROM entities              WHERE id::text        LIKE '00000089-%';

-- Leave the config as 0089 migrated it, so a file running after this one sees the
-- shipped state rather than this file's fixtures. 0078 does the same in reverse and
-- that is why it has to be set at the top rather than assumed.
UPDATE entity_ranking_config
   SET participation_weight = 2.5, participation_cap = 12, tau_seconds = 100000
 WHERE id;

SELECT 'ALL 0089 ASSERTIONS PASSED' AS result;
