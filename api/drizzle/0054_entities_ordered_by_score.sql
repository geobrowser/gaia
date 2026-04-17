DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'score_type') THEN
        CREATE TYPE score_type AS ENUM ('local', 'global', 'raw');
    END IF;
END $$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_ordered_by_score(
  score_type score_type,
  space_id uuid DEFAULT NULL,
  sort_direction sort_order DEFAULT 'DESC'
)
RETURNS SETOF public.entities
LANGUAGE plpgsql STABLE AS $$
BEGIN
  -- ERRCODE 22023 (invalid_parameter_value) marks these as user input errors so the
  -- API layer can surface the message to clients instead of masking it.
  IF score_type IS NULL THEN
    RAISE EXCEPTION 'score_type is required'
      USING ERRCODE = '22023';
  END IF;

  -- space_id is still required for local scores (they are inherently per-space).
  -- For raw, a NULL space_id means "sum across all spaces".
  IF score_type = 'local' AND space_id IS NULL THEN
    RAISE EXCEPTION 'space_id is required for score_type: %', score_type
      USING ERRCODE = '22023';
  END IF;

  IF score_type = 'local' THEN
    RETURN QUERY
      SELECT e.*
      FROM entities e
      INNER JOIN local_scores ls ON ls.entity_id = e.id AND ls.space_id = entities_ordered_by_score.space_id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN ls.score END ASC,
        CASE WHEN sort_direction = 'DESC' THEN ls.score END DESC,
        e.id ASC;

  ELSIF score_type = 'global' THEN
    RETURN QUERY
      SELECT e.*
      FROM entities e
      INNER JOIN global_scores gs ON gs.entity_id = e.id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN gs.score END ASC,
        CASE WHEN sort_direction = 'DESC' THEN gs.score END DESC,
        e.id ASC;

  ELSIF score_type = 'raw' AND entities_ordered_by_score.space_id IS NOT NULL THEN
    RETURN QUERY
      SELECT e.*
      FROM entities e
      INNER JOIN votes_count vc ON vc.object_id = e.id AND vc.object_type = 0 AND vc.space_id = entities_ordered_by_score.space_id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN (vc.upvotes - vc.downvotes) END ASC,
        CASE WHEN sort_direction = 'DESC' THEN (vc.upvotes - vc.downvotes) END DESC,
        e.id ASC;

  ELSIF score_type = 'raw' THEN
    RETURN QUERY
      SELECT e.*
      FROM entities e
      INNER JOIN (
        SELECT vc.object_id, SUM(vc.upvotes - vc.downvotes)::bigint AS net_score
        FROM votes_count vc
        WHERE vc.object_type = 0
        GROUP BY vc.object_id
      ) agg ON agg.object_id = e.id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN agg.net_score END ASC,
        CASE WHEN sort_direction = 'DESC' THEN agg.net_score END DESC,
        e.id ASC;
  END IF;
END;
$$;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_global_scores_score ON global_scores (score DESC);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_votes_count_space_net_score ON votes_count (space_id, (upvotes - downvotes) DESC) WHERE object_type = 0;
