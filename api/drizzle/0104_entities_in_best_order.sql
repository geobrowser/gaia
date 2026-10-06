-- GEO-3150: Best's order for any list of entities — for example the claims a question groups,
-- or a debate lobby's claim list.
--
-- Best's scores were already public (`Entity.rankingScore`), but the only ordered read was
-- Explore's own candidate set (`entities_ranked_for_feed`). This orders exactly the entities the
-- caller names, the way Best orders: `ranking_score` DESC, ties by entity id DESC.
--
-- Unlike the feed, nothing is hidden. The caller chose these ids for a specific page, so an
-- entity without a score comes back after every scored one, in the order the caller gave it,
-- rather than being dropped; and Best's feed-only filters (blocklist, system and excluded types,
-- unrenderable names) are not applied. Ids that do not exist are omitted, and duplicates are
-- returned once.
--
-- Bounded at 1000 ids so a single request stays cheap.
CREATE OR REPLACE FUNCTION public.entities_in_best_order(entity_ids uuid[])
RETURNS SETOF public.entities
LANGUAGE plpgsql STABLE PARALLEL SAFE AS $$
BEGIN
	IF coalesce(array_length(entity_ids, 1), 0) > 1000 THEN
		RAISE EXCEPTION 'entitiesInBestOrder accepts at most 1000 ids (got %)', array_length(entity_ids, 1)
			USING ERRCODE = '22023';
	END IF;

	RETURN QUERY
	SELECT e.*
	FROM (
		-- First position of each id, so a duplicate does not move it.
		SELECT DISTINCT ON (t.id) t.id, t.pos
		FROM unnest(entity_ids) WITH ORDINALITY AS t(id, pos)
		ORDER BY t.id, t.pos
	) requested
	JOIN public.entities e ON e.id = requested.id
	LEFT JOIN public.entity_ranking_scores rs ON rs.entity_id = e.id
	ORDER BY
		rs.ranking_score DESC NULLS LAST,
		-- Scored ties break as Best breaks them; unscored entities keep the caller's order.
		CASE WHEN rs.entity_id IS NULL THEN requested.pos END,
		e.id DESC;
END;
$$;
