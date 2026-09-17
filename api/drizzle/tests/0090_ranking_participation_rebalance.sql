-- Assertions for 0090: participation is worth about ten days, not six and not eighteen.
--
-- Run per drizzle/tests/README.md. Removes only its own fixtures, so it passes in any
-- order and is safe against a database that holds real rows.
--
-- 0089 is the reason this file brackets the value from BOTH sides. Its assertions all
-- passed, its own measurements were correct, and it still shipped a regression: it
-- proved participation had been weakened without ever asking whether it had been
-- weakened too far. A one-sided test cannot catch that, because "engagement counts for
-- less" is exactly what such a test is written to confirm.
--
-- So the two central assertions are a pair, and each flips against a DIFFERENT
-- parameter set:
--   1. 8 votes still outrank 10 days of recency.  Fails at w2.5 (the shipped regression).
--   2. 8 votes do NOT outrank 14 days.            Fails at w7  (the original complaint).
-- Together they pin the term into a 10-14 day band rather than merely below 18 days,
-- and no single-direction mistake can satisfy both. Both flips were verified by hand,
-- by re-running this file with the config set to w2.5/cap12 and to w7/cap30.
--
-- THE GAPS ARE NOT `participation / 0.864`, and getting that wrong is how this file
-- first went green under both parameter sets. A well-voted entity also gains on the
-- Wilson QUALITY term — 8 positive votes are worth ~1.84 units against an unvoted
-- entity's prior — so the bonus being bracketed is participation + quality:
--
--   weight   participation   + quality   = bonus   outranks
--     2.5            5.493        1.841     7.334     8.5 d
--     4.0            8.789        1.841    10.630    12.3 d
--     7.0           15.383        1.841    17.224    19.9 d
--
-- An 8-day gap therefore proves nothing: the voted entity wins it at w2.5 as well.
--
-- What is NOT testable here: the actual metric that failed, which is the TYPE
-- COMPOSITION of the rendered feed (Claim 15 -> 5 of a 22-row page, News story 3 -> 16).
-- That needs the ranked connection, the diversity cap and the per-space quota, none of
-- which exist at the SQL layer. The band above is its parameter-space proxy, and the
-- measurement itself lives in the migration header. If this file ever passes while the
-- feed is wrong again, that gap is where to look first.

\set ON_ERROR_STOP on

-- `IS NOT TRUE`, not `NOT cond`. Under `NOT cond` a NULL condition is neither true nor
-- false, so it takes the ELSE branch and reports a PASS — the vacuous-assertion trap in
-- README.md. A `SELECT ... INTO` that matches no row leaves its variable NULL, and the
-- assertion below it then proves nothing while printing "pass".
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

-- Scoped cleanup rather than TRUNCATE, for the reason 0089 documents: `spaces` and
-- `subspace_topics` now carry foreign keys to `entities`, so the truncate lists in
-- 0078/0079/0083/0084 fail outright and those files no longer run at all.
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000090-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000090-%';
DELETE FROM entities              WHERE id::text        LIKE '00000090-%';

-- The values 0090 writes, SET explicitly rather than read back — `entity_ranking_config`
-- is one shared row that several test files mutate without restoring, so asserting the
-- migrated values would pass alone and fail in a suite.
UPDATE entity_ranking_config
   SET participation_weight = 4.0, participation_cap = 19,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. One day of age = tau/86400 = 0.864 score units.
-- 8 curation votes -> participation 4.0 * ln(9) = 8.789, plus ~1.841 of quality = 10.630.
--
--   NEWEST      now, no votes                  -> the thing everything is compared to
--   VOTED_10D   10 days older, 8 votes         -> 10.630 vs  8.640 => WINS  (fails at w2.5)
--   VOTED_14D   14 days older, 8 votes         -> 10.630 vs 12.096 => LOSES (fails at w7)
--   SAME_AGE    now, 8 votes                   -> votes still decide at equal age
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES
  ('00000090-0000-4000-8000-000000000001', extract(epoch from now())::bigint::text, '0',
                                            extract(epoch from now())::bigint::text, '0'),
  ('00000090-0000-4000-8000-000000000002', (extract(epoch from now())::bigint - 864000)::text, '0',
                                            (extract(epoch from now())::bigint - 864000)::text, '0'),
  ('00000090-0000-4000-8000-000000000003', (extract(epoch from now())::bigint - 1209600)::text, '0',
                                            (extract(epoch from now())::bigint - 1209600)::text, '0'),
  ('00000090-0000-4000-8000-000000000004', extract(epoch from now())::bigint::text, '0',
                                            extract(epoch from now())::bigint::text, '0');

-- vote_kind 0 is curation, which 0083 made count toward participation alongside stance.
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('00000090-0000-4000-8000-000000000002', 0, '00000090-0000-4000-8000-0000000000aa', 0, 8, 0),
  ('00000090-0000-4000-8000-000000000003', 0, '00000090-0000-4000-8000-0000000000aa', 0, 8, 0),
  ('00000090-0000-4000-8000-000000000004', 0, '00000090-0000-4000-8000-0000000000aa', 0, 8, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY[
  '00000090-0000-4000-8000-000000000001',
  '00000090-0000-4000-8000-000000000002',
  '00000090-0000-4000-8000-000000000003',
  '00000090-0000-4000-8000-000000000004'
]::uuid[]);

-- 1. THE REGRESSION 0089 SHIPPED. 8 votes must still outrank 10 days of recency.
--    At w2.5 the bonus is 7.33 against an 8.64 age gap and this FAILS — which is the
--    feed Preston saw: the types that earn votes stopped being able to hold a page
--    against whatever was newest.
DO $$
DECLARE voted_10d numeric; newest numeric;
BEGIN
  SELECT ranking_score INTO voted_10d FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000002';
  SELECT ranking_score INTO newest FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000001';
  PERFORM assert(voted_10d > newest,
    format('8 votes no longer outrank 10 days of recency (%s vs %s) — participation has '
           'been weakened too far and the feed reverts to chronological, which the most '
           'numerous recent type wins by default', voted_10d, newest));
END $$;

-- 2. THE ORIGINAL COMPLAINT. 8 votes must NOT outrank 14 days of recency.
--    At w7 participation is 15.38 against a 12.10 age gap and this FAILS — the
--    entrenchment Yaniv reported, where the same debates hold the top of a space.
DO $$
DECLARE voted_14d numeric; newest numeric;
BEGIN
  SELECT ranking_score INTO voted_14d FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000003';
  SELECT ranking_score INTO newest FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000001';
  PERFORM assert(voted_14d < newest,
    format('8 votes outrank a fortnight of recency (%s vs %s) — engagement is buying '
           'weeks again and entrenchment returns', voted_14d, newest));
END $$;

-- 3. Votes still decide between entities of the same age.
DO $$
DECLARE voted numeric; quiet numeric;
BEGIN
  SELECT ranking_score INTO voted FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000004';
  SELECT ranking_score INTO quiet FROM entity_ranking_scores
   WHERE entity_id = '00000090-0000-4000-8000-000000000001';
  PERFORM assert(voted > quiet,
    format('at identical age, 8 votes no longer outrank 0 (%s vs %s)', voted, quiet));
END $$;

-- 4. The cap stays proportional: it binds at ~114 votes, as 12 did at weight 2.5.
--    Left at 12 it would bind at ~42, and the most-voted entity in the live corpus has
--    ~43 — the ceiling would have started clipping the single most engaged item without
--    anyone deciding that.
DO $$
DECLARE capped numeric; at_43 numeric; at_113 numeric; at_200 numeric;
BEGIN
  SELECT participation_cap INTO capped FROM entity_ranking_config WHERE id;
  at_43  := public.entity_participation_score(43,  4.0, capped);
  at_113 := public.entity_participation_score(113, 4.0, capped);
  at_200 := public.entity_participation_score(200, 4.0, capped);
  PERFORM assert(at_43 < capped,
    format('the most-voted entity in the live corpus (~43 votes) is clipped by the cap '
           '(%s = %s) — the ceiling has become a routine constraint', at_43, capped));
  PERFORM assert(at_113 < capped,
    format('113 votes is below the cap (%s < %s)', at_113, capped));
  PERFORM assert(at_200 = capped,
    format('200 votes reaches the cap (%s = %s)', at_200, capped));
END $$;

DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000090-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000090-%';
DELETE FROM entities              WHERE id::text        LIKE '00000090-%';

-- Leave the config as 0090 migrated it, so a file running after this one sees the
-- shipped state rather than this file's fixtures.
UPDATE entity_ranking_config
   SET participation_weight = 4.0, participation_cap = 19, tau_seconds = 100000
 WHERE id;

SELECT 'ALL 0090 ASSERTIONS PASSED' AS result;
