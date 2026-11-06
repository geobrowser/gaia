-- GEO-3556: Debate agenda listings for the analytics warehouse, so "what share of debates were on an
-- agenda claim" is one warehouse query. ranking-indexer's agenda_listings_refresh sends them right
-- after each hourly refresh of trending.agenda_listings (0113), to analytics.gaia_agenda_listings
-- (analytics migration 124), as a ClickHouse user that can only INSERT there.
--
-- HISTORY IS KEPT. The refresh deletes a row when its agenda is edited (a claim dropped, the agenda
-- re-dated or deleted), but a debate on that claim before the edit was still on the agenda. So the
-- warehouse is never told to forget: a row is sent once when it appears, and once more, marked
-- removed_at, when it disappears. The writer can only add rows and cannot read the warehouse, so
-- what it has sent is recorded here.
--
-- A listing is (agenda_id, space_id, entity_id, source, listed_on, written_at): written_at is when
-- the refresh first saw the row and never changes while the row stands; a row the refresh deletes
-- and writes again (a re-dated agenda, a claim dropped then re-added) is a new listing with its own
-- written_at. So each listing is one interval, [written_at, removed_at).

CREATE TABLE IF NOT EXISTS "trending"."agenda_listings_sent" (
  "agenda_id" uuid NOT NULL,
  "space_id" uuid NOT NULL,
  "entity_id" uuid NOT NULL,
  "source" text NOT NULL,
  "listed_on" date NOT NULL,
  "written_at" timestamptz NOT NULL,
  "removed_at" timestamptz,                                                -- NULL: still listed
  "sent_at" timestamptz NOT NULL,                                          -- the last send of the row
  PRIMARY KEY ("agenda_id", "space_id", "entity_id", "source", "listed_on", "written_at")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "agenda_listings_sent_open_idx" ON "trending"."agenda_listings_sent"
  ("agenda_id", "space_id", "entity_id", "source") WHERE "removed_at" IS NULL;
--> statement-breakpoint

-- What the warehouse has not been told yet, recorded as told: listings new since the last call
-- (removed_at NULL) and listings sent before that are gone now (removed_at = run_at). Call it in a
-- transaction and commit only once the warehouse has the rows, so a failed send is offered again
-- by the next call. The first call returns every listing.
CREATE OR REPLACE FUNCTION trending.take_agenda_listing_changes(run_at timestamptz DEFAULT now())
RETURNS TABLE (
  agenda_id uuid,
  space_id uuid,
  entity_id uuid,
  source text,
  listed_on date,
  written_at timestamptz,
  removed_at timestamptz
)
LANGUAGE sql AS $$
  -- One sender at a time, so two never both claim the same change.
  SELECT pg_advisory_xact_lock(hashtext('trending.agenda_listings_sent'));

  WITH added AS (
    INSERT INTO trending.agenda_listings_sent AS s
      (agenda_id, space_id, entity_id, source, listed_on, written_at, removed_at, sent_at)
    SELECT l.agenda_id, l.space_id, l.entity_id, l.source, l.listed_on, l.written_at, NULL, run_at
    FROM trending.agenda_listings l
    ON CONFLICT DO NOTHING
    RETURNING s.agenda_id, s.space_id, s.entity_id, s.source, s.listed_on, s.written_at, s.removed_at
  ),
  gone AS (
    UPDATE trending.agenda_listings_sent s
    SET removed_at = run_at, sent_at = run_at
    WHERE s.removed_at IS NULL
      AND NOT EXISTS (
        SELECT 1 FROM trending.agenda_listings l
        WHERE (l.agenda_id, l.space_id, l.entity_id, l.source, l.listed_on, l.written_at)
            = (s.agenda_id, s.space_id, s.entity_id, s.source, s.listed_on, s.written_at)
      )
    RETURNING s.agenda_id, s.space_id, s.entity_id, s.source, s.listed_on, s.written_at, s.removed_at
  )
  SELECT * FROM added
  UNION ALL
  SELECT * FROM gone;
$$;
