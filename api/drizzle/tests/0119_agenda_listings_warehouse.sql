-- Assertions for 0119: Debate agenda listings for the analytics warehouse (GEO-3556).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. The first call returns every listing, topic listings too, none removed, and records them.
--   2. A call with nothing changed returns nothing.
--   3. A listing the refresh deleted comes back once, removed at the call's time; never again.
--   4. A re-dated listing is the old one removed and the new one added: two rows.
--   5. A claim dropped and re-added on the same Date is two listings: the first one's removal and
--      the second's addition, each with its own written_at.
--   6. Changes taken in a transaction that rolls back (the warehouse refused them) are offered again.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE trending.agenda_listings, trending.agenda_listings_sent;

-- A1 lists claims C1 and C2 in a block and topic T1; A2 lists C3 through its Claims relation.
INSERT INTO trending.agenda_listings (agenda_id, space_id, entity_id, listed_on, source, written_at) VALUES
  ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000f1', '00000000-0000-0000-0000-0000000000c1', '2026-11-03', 'block_item',    '2026-11-03 05:05+00'),
  ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000f1', '00000000-0000-0000-0000-0000000000c2', '2026-11-03', 'block_item',    '2026-11-03 05:05+00'),
  ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000f1', '00000000-0000-0000-0000-0000000000d1', '2026-11-03', 'agenda_topics', '2026-11-03 05:05+00'),
  ('00000000-0000-0000-0000-0000000000a2', '00000000-0000-0000-0000-0000000000f2', '00000000-0000-0000-0000-0000000000c3', '2026-11-03', 'agenda_claims', '2026-11-03 06:05+00');

CREATE TEMP TABLE taken AS
SELECT * FROM trending.take_agenda_listing_changes('2026-11-03 07:05+00');

-- 1
SELECT assert((SELECT count(*) FROM taken) = 4, '1: the first call returns every listing');
SELECT assert((SELECT count(*) FROM taken WHERE removed_at IS NULL) = 4, '1: none of them removed');
SELECT assert(EXISTS (SELECT 1 FROM taken WHERE source = 'agenda_topics'), '1: topic listings are sent too');
SELECT assert((SELECT written_at FROM taken WHERE entity_id = '00000000-0000-0000-0000-0000000000c3') = '2026-11-03 06:05+00',
              '1: each listing keeps the time the refresh first saw it');
SELECT assert((SELECT count(*) FROM trending.agenda_listings_sent WHERE sent_at = '2026-11-03 07:05+00') = 4,
              '1: and they are recorded as sent');

-- 2
SELECT assert((SELECT count(*) FROM trending.take_agenda_listing_changes('2026-11-03 08:05+00')) = 0,
              '2: nothing changed, nothing returned');

-- 3: the refresh drops C2 from A1.
DELETE FROM trending.agenda_listings WHERE entity_id = '00000000-0000-0000-0000-0000000000c2';
TRUNCATE taken;
INSERT INTO taken SELECT * FROM trending.take_agenda_listing_changes('2026-11-03 09:05+00');
SELECT assert((SELECT count(*) FROM taken) = 1, '3: one change');
SELECT assert((SELECT entity_id = '00000000-0000-0000-0000-0000000000c2' AND removed_at = '2026-11-03 09:05+00'
                      AND written_at = '2026-11-03 05:05+00' AND listed_on = '2026-11-03' FROM taken),
              '3: the dropped listing, as sent before, removed at the call''s time');
SELECT assert((SELECT count(*) FROM trending.take_agenda_listing_changes('2026-11-03 10:05+00')) = 0,
              '3: a removal is sent once');

-- 4: A2 is re-dated to 4 Nov; the refresh writes its listing again.
UPDATE trending.agenda_listings SET listed_on = '2026-11-04', written_at = '2026-11-03 11:05+00'
WHERE agenda_id = '00000000-0000-0000-0000-0000000000a2';
TRUNCATE taken;
INSERT INTO taken SELECT * FROM trending.take_agenda_listing_changes('2026-11-03 11:05+00');
SELECT assert((SELECT count(*) FROM taken) = 2, '4: a re-dated listing is two changes');
SELECT assert((SELECT removed_at FROM taken WHERE listed_on = '2026-11-03') = '2026-11-03 11:05+00',
              '4: the old Date removed');
SELECT assert((SELECT removed_at IS NULL FROM taken WHERE listed_on = '2026-11-04'), '4: the new Date added');

-- 5: C1 is dropped, then added back to the same agenda and Date.
DELETE FROM trending.agenda_listings WHERE entity_id = '00000000-0000-0000-0000-0000000000c1';
INSERT INTO trending.agenda_listings (agenda_id, space_id, entity_id, listed_on, source, written_at) VALUES
  ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000f1', '00000000-0000-0000-0000-0000000000c1', '2026-11-03', 'block_item', '2026-11-03 12:05+00');
TRUNCATE taken;
INSERT INTO taken SELECT * FROM trending.take_agenda_listing_changes('2026-11-03 12:05+00');
SELECT assert((SELECT count(*) FROM taken WHERE entity_id = '00000000-0000-0000-0000-0000000000c1') = 2,
              '5: dropped and re-added is two listings');
SELECT assert((SELECT removed_at FROM taken WHERE written_at = '2026-11-03 05:05+00') = '2026-11-03 12:05+00',
              '5: the first one ends');
SELECT assert((SELECT removed_at IS NULL FROM taken WHERE written_at = '2026-11-03 12:05+00'),
              '5: the second one starts');
SELECT assert((SELECT count(*) FROM trending.agenda_listings_sent
               WHERE entity_id = '00000000-0000-0000-0000-0000000000c1') = 2,
              '5: both are kept');

-- 6: the warehouse refuses a send, so its transaction rolls back.
DELETE FROM trending.agenda_listings WHERE entity_id = '00000000-0000-0000-0000-0000000000d1';
DO $$
BEGIN
  PERFORM trending.take_agenda_listing_changes('2026-11-03 13:05+00');
  RAISE EXCEPTION 'warehouse refused';
EXCEPTION WHEN raise_exception THEN NULL;
END $$;
TRUNCATE taken;
INSERT INTO taken SELECT * FROM trending.take_agenda_listing_changes('2026-11-03 14:05+00');
SELECT assert((SELECT count(*) FROM taken) = 1 AND (SELECT removed_at FROM taken) = '2026-11-03 14:05+00',
              '6: a change whose send failed is offered again');

DROP TABLE taken;
TRUNCATE trending.agenda_listings, trending.agenda_listings_sent;
