-- GEO-3088 (last item): "Not interested" on claims counts against the claim's topics.
--
-- WHERE IT LIVES. Not interested (GEO-2862) is stored in geo-chat, off-chain and private:
-- `claim_not_interested (user_id, claim_entity_id, created_at)`, keyed on geo-chat's user id, whose
-- `users.profile_space_id` is the person's Geo personal space — the same id gaia keys a person on
-- (user_votes.user_id). Nothing about it is in the graph or in gaia's database, so it has to be
-- carried across.
--
-- HOW IT GETS HERE. ranking-indexer's `not_interested_sync` CronJob reads geo-chat's table through a
-- read-only role every couple of minutes and hands the WHOLE current set to
-- `replace_external_interest_signals` below. A snapshot, not a stream of mark/clear events, so
-- there is nothing to lose: an undo in geo-chat is a row missing from the next snapshot, a missed
-- run is repaired by the next one, and a backfill is the first run.
--
-- WHERE IT GOES. 0100 already defines the destination: `personalization.external_interest_signals`
-- (kind 'not_interested', weight −3.0, 30-day half-life), which `user_interest_events` reads, so a
-- Not interested lowers the user's weight for each topic of the claim by the same route a vote
-- raises it. Spreading is untouched: 0100 spreads only positive interest.
--
-- FRESHNESS. The sweep keys on `recorded_at`, which sees a new row but not a removed one, so this
-- function recomputes every user whose set changed itself, in the same transaction, under the lock
-- the sweep and refit use. A user's weights move within one sync of a mark or an undo.
--
-- ALSO: a claim the user marked Not interested is left out of their For you window (0102's
-- `for_you_candidates` gains the reason 'not_interested'), as a held position already is.
--
-- PRIVACY. Private schema, which PostGraphile does not introspect; the function is also @omit and
-- listed in HIDDEN_SURFACE.

-- Replace every row of one kind with `p_rows`, a JSON array of
-- {"user_id": uuid, "object_id": uuid, "occurred_at": timestamptz}. Rows whose user is not a
-- Personal space are dropped (and counted): the person has no personal space in gaia yet, and the
-- next snapshot inserts them once they do. Then recompute every user whose rows changed.
--
-- An empty snapshot is honoured: everyone cleared their marks. But a NON-empty snapshot that places
-- no row at all, while rows exist, is refused unless p_allow_unplaced: that is the identity mapping
-- broken (geo-chat's profile_space_id no longer a gaia personal space), and acting on it would
-- erase everyone's Not interested. Returns what it did.
CREATE OR REPLACE FUNCTION personalization.replace_external_interest_signals(
  p_kind text,
  p_rows jsonb,
  p_allow_unplaced boolean DEFAULT false,
  as_of timestamptz DEFAULT now()
)
RETURNS TABLE (rows_in integer, placed integer, inserted integer, updated integer, removed integer,
               users_recomputed integer, signal_rows integer)
LANGUAGE plpgsql AS $$
DECLARE
  n_in integer;
  n_placed integer;
  n_existing integer;
  n_inserted integer;
  n_updated integer;
  n_removed integer;
  changed uuid[];
  written integer;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM personalization.interest_signal_weights w WHERE w.kind = p_kind) THEN
    RAISE EXCEPTION 'unknown interest signal kind %', p_kind;
  END IF;
  IF p_rows IS NULL OR jsonb_typeof(p_rows) <> 'array' THEN
    RAISE EXCEPTION 'p_rows must be a JSON array';
  END IF;

  -- Waits for a running sweep or refit: both recompute users from this table.
  PERFORM pg_advisory_xact_lock(hashtext('personalization.user_topic_interest'));

  CREATE TEMP TABLE IF NOT EXISTS incoming_signals (
    user_id uuid, object_id uuid, occurred_at timestamptz, PRIMARY KEY (user_id, object_id)
  ) ON COMMIT DROP;
  TRUNCATE incoming_signals;

  n_in := jsonb_array_length(p_rows);
  -- One row per (user, object); if a source ever sends two, the earlier mark wins, as geo-chat's
  -- own re-mark keeps the first time.
  INSERT INTO incoming_signals (user_id, object_id, occurred_at)
  SELECT DISTINCT ON (r.user_id, r.object_id) r.user_id, r.object_id, r.occurred_at
  FROM jsonb_to_recordset(p_rows) AS r(user_id uuid, object_id uuid, occurred_at timestamptz)
  WHERE r.user_id IS NOT NULL AND r.object_id IS NOT NULL AND r.occurred_at IS NOT NULL
    AND EXISTS (SELECT 1 FROM public.spaces s WHERE s.id = r.user_id AND s.type = 'Personal')
  ORDER BY r.user_id, r.object_id, r.occurred_at;
  GET DIAGNOSTICS n_placed = ROW_COUNT;

  SELECT count(*) INTO n_existing FROM personalization.external_interest_signals x WHERE x.kind = p_kind;
  IF n_in > 0 AND n_placed = 0 AND n_existing > 0 AND NOT p_allow_unplaced THEN
    RAISE EXCEPTION 'refusing to remove all % % signals: none of the snapshot''s % rows is on a personal space',
      n_existing, p_kind, n_in;
  END IF;

  CREATE TEMP TABLE IF NOT EXISTS changed_signal_users (user_id uuid) ON COMMIT DROP;
  TRUNCATE changed_signal_users;

  WITH gone AS (
    DELETE FROM personalization.external_interest_signals x
    WHERE x.kind = p_kind
      AND NOT EXISTS (SELECT 1 FROM incoming_signals i
                      WHERE i.user_id = x.user_id AND i.object_id = x.object_id)
    RETURNING x.user_id
  )
  INSERT INTO changed_signal_users SELECT user_id FROM gone;
  GET DIAGNOSTICS n_removed = ROW_COUNT;

  -- A mark cleared and made again between two syncs comes back with a new time.
  WITH moved AS (
    UPDATE personalization.external_interest_signals x
    SET occurred_at = i.occurred_at, recorded_at = now()
    FROM incoming_signals i
    WHERE x.kind = p_kind AND x.user_id = i.user_id AND x.object_id = i.object_id
      AND x.occurred_at IS DISTINCT FROM i.occurred_at
    RETURNING x.user_id
  )
  INSERT INTO changed_signal_users SELECT user_id FROM moved;
  GET DIAGNOSTICS n_updated = ROW_COUNT;

  WITH added AS (
    INSERT INTO personalization.external_interest_signals (user_id, object_id, kind, occurred_at)
    SELECT i.user_id, i.object_id, p_kind, i.occurred_at FROM incoming_signals i
    ON CONFLICT (user_id, object_id, kind) DO NOTHING
    RETURNING user_id
  )
  INSERT INTO changed_signal_users SELECT user_id FROM added;
  GET DIAGNOSTICS n_inserted = ROW_COUNT;

  SELECT coalesce(array_agg(DISTINCT c.user_id), '{}') INTO changed FROM changed_signal_users c;

  written := personalization.recompute_user_topic_interest(changed, as_of);

  RETURN QUERY SELECT n_in, n_placed, n_inserted, n_updated, n_removed, cardinality(changed), written;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.replace_external_interest_signals(text, jsonb, boolean, timestamptz) IS E'@omit';
--> statement-breakpoint

-- 0102's candidate read, with one more reason to leave a candidate out: the user marked it Not
-- interested. Same order of checks otherwise; a held position still reads 'voted'.
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
           WHEN EXISTS (
             SELECT 1 FROM personalization.external_interest_signals s
             WHERE s.user_id = p_user_id AND s.object_id = c.id AND s.kind = 'not_interested'
           ) THEN 'not_interested'
         END
  FROM unnest(p_candidate_ids) WITH ORDINALITY AS c(id, ord)
  LEFT JOIN public.entity_ranking_scores ers ON ers.entity_id = c.id
  ORDER BY c.ord
$$;
--> statement-breakpoint

UPDATE personalization.interest_signal_weights
   SET note = 'Not interested on a claim or question (GEO-2862), synced from geo-chat by not_interested_sync.'
 WHERE kind = 'not_interested';
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
