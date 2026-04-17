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
  -- Validate space_id for local and raw
  IF score_type IN ('local', 'raw') AND space_id IS NULL THEN
    RAISE EXCEPTION 'space_id is required for score_type: %', score_type;
  END IF;

  IF score_type = 'local' THEN
    RETURN QUERY
      SELECT DISTINCT e.*
      FROM entities e
      INNER JOIN local_scores ls ON ls.entity_id = e.id AND ls.space_id = entities_ordered_by_score.space_id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN ls.score END ASC,
        CASE WHEN sort_direction = 'DESC' THEN ls.score END DESC,
        e.id ASC;

  ELSIF score_type = 'global' THEN
    RETURN QUERY
      SELECT DISTINCT e.*
      FROM entities e
      INNER JOIN global_scores gs ON gs.entity_id = e.id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN gs.score END ASC,
        CASE WHEN sort_direction = 'DESC' THEN gs.score END DESC,
        e.id ASC;

  ELSIF score_type = 'raw' THEN
    RETURN QUERY
      SELECT DISTINCT e.*
      FROM entities e
      INNER JOIN votes_count vc ON vc.object_id = e.id AND vc.object_type = 0 AND vc.space_id = entities_ordered_by_score.space_id
      ORDER BY
        CASE WHEN sort_direction = 'ASC' THEN (vc.upvotes - vc.downvotes) END ASC,
        CASE WHEN sort_direction = 'DESC' THEN (vc.upvotes - vc.downvotes) END DESC,
        e.id ASC;
  END IF;
END;
$$;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_global_scores_score ON global_scores (score DESC);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_votes_count_space_net_score ON votes_count (space_id, (upvotes - downvotes) DESC) WHERE object_type = 0;
