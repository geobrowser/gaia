-- Stop writing personalization.interest_refit_runs and personalization.stance_map_runs.
-- Nothing reads either table (no query, route, dashboard or alert in gaia or analytics), so the
-- rows are dead weight. Both writes live inside these two functions, so the only way to stop them
-- is to replace the functions. Signatures and return values are unchanged: the refit still returns
-- its disagreement report and record_stance_map_run still validates and returns the gate, and the
-- callers log both. The tables stay in place, untouched; dropping them is a separate later change.

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

  RETURN QUERY SELECT n_users, n_rows, n_disagree, max_diff;
END;
$$;
--> statement-breakpoint

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

	RETURN met;
END;
$$;
