-- GEO-3158: Interested, a public vote kind (vote_kind 3) that is also the topic follow.
--
-- THE KIND. Two permissionless actions, PERMISSIONLESS.INTERESTED and PERMISSIONLESS.UNINTERESTED,
-- arrive through hermes-pipeline as (vote_kind 3, Up) and (vote_kind 3, None). There is no Down:
-- "not interested" is a separate, private signal (GEO-2862), not an on-chain response. The
-- vote-indexer writes them to `votes`, `user_votes` and `votes_count` like any other kind, keyed by
-- kind, so no table change is needed: `vote_kind` is a smallint with no CHECK on its range (0071),
-- and an Interested's `negative` tally simply stays 0. GraphQL exposes it as it exposes stance:
-- `votesCountsConnection(condition: {objectId, voteKind: 3})` for counts and
-- `userVotesConnection(condition: {userId, objectId, voteKind: 3})` for the viewer's own state.
--
-- WHAT MUST NOT COUNT IT. Every reader of votes_count / user_votes was checked against an
-- Interested arriving (vote_type 0, kind 3). The ranking (refresh_entity_ranking_scores: kinds 0
-- and 1), Best and Top (entities_ordered_by_score, valueOrderByScorePlugin: kind 0), the vote
-- notifications (kind 0), the score `values` row (kind 0) and every stance reader (kind 1) are
-- already scoped by kind. The two that were not, and would have read a follow as a vote, are fixed
-- here and in the scoring cronjob:
--   * user_interest_events read "any kind, either direction" as a 'vote' at weight 1.0, decaying.
--   * scoring-service/cronjob read every user_votes row as an up/down vote (outside this file).
--
-- THE INTEREST MODEL. Interested on a Topic IS the follow (Preston, 6 Oct): kind 'follow', the
-- follow weight (3.0), non-decaying, one event per (user, topic). The `Following` relation is no
-- longer a follow source at all: old topic follows are dropped, not migrated. A topic is followed
-- exactly when the user holds a current Interested on it; clearing it (vote_type 2) unfollows.
--
-- DROPPING OLD FOLLOWS takes effect user by user as each is recomputed: on their next sweep if they
-- do anything, and for everyone at the first nightly refit after deploy, which will report those
-- rows as disagreement (expected, once). dirty_interest_users still marks a user whose Following
-- relation changes; that recompute is now a no-op for follows and is left alone here.
--
-- Interested on something that is not a Topic stays what it was before this migration: an
-- engagement with that entity, credited to its topics at the vote weight. Only topics carry the
-- Interested control today, so this path is for completeness, not a product decision.
--
-- The 'interested' row in interest_signal_weights was the planned Interested-on-a-question signal,
-- fed through external_interest_signals. Nothing has ever written it; the question debate now uses
-- topics, so Interested on a topic (above) replaces it. The row stays because
-- for_you_candidates (0105) and api/src/forYou/rank.ts still name the kind; its note says so.
--
-- FRESHNESS needs no change: dirty_interest_users already marks any user with a user_votes row
-- newer than the sweep cursor, and an Interested or its clearing moves voted_at.

CREATE OR REPLACE FUNCTION personalization.user_interest_events(user_ids uuid[])
RETURNS TABLE (user_id uuid, kind text, source_id uuid, object_id uuid, object_is_topic boolean,
               occurred_at timestamptz)
LANGUAGE sql STABLE AS $$
  WITH RECURSIVE
  users AS (
    -- Only Personal spaces are people. A DAO space can author a comment or be named somewhere,
    -- and none of that is one person's interest.
    SELECT s.id FROM public.spaces s
    WHERE s.id = ANY(user_ids) AND s.type = 'Personal'
  ),
  comments AS (
    SELECT c.space_id AS user_id, c.from_entity_id AS comment_id, c.to_entity_id AS target
    FROM public.relations c
    JOIN users u ON u.id = c.space_id
    WHERE c.type_id = '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid
  ),
  -- A reply to a comment is about whatever the thread is about: walk up the Reply to chain until
  -- the target is not itself a comment. Bounded, in case the graph ever holds a cycle.
  thread(user_id, comment_id, target, depth) AS (
    SELECT user_id, comment_id, target, 0 FROM comments
    UNION ALL
    SELECT t.user_id, t.comment_id, up.to_entity_id, t.depth + 1
    FROM thread t
    CROSS JOIN LATERAL (
      SELECT r.to_entity_id FROM public.relations r
      WHERE r.from_entity_id = t.target AND r.type_id = '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid
      ORDER BY r.to_entity_id LIMIT 1
    ) up
    WHERE t.depth < 20
  ),
  comment_roots AS (
    SELECT DISTINCT ON (user_id, comment_id) user_id, comment_id, target
    FROM thread ORDER BY user_id, comment_id, depth DESC
  ),
  -- A user's held responses on entities: not removals (vote_type 2), people only.
  held AS (
    SELECT v.user_id, v.object_id, v.vote_kind, v.voted_at,
           -- Whether the object is typed Topic. Only needed for Interested, so only looked up there.
           v.vote_kind = 3 AND EXISTS (
             SELECT 1 FROM public.relations ty
             WHERE ty.from_entity_id = v.object_id
               AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
               AND ty.to_entity_id = '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid
           ) AS interested_in_topic
    FROM public.user_votes v
    WHERE v.user_id = ANY(user_ids) AND v.object_type = 0 AND v.vote_type IN (0, 1)
      AND EXISTS (SELECT 1 FROM users u WHERE u.id = v.user_id)
  ),
  -- Follows: a current Interested on a Topic (GEO-3158), and nothing else. Interested only ever
  -- holds vote_type 0, so `held` (which drops removals) is exactly the current Interesteds. One row
  -- per (user, topic), however many spaces the Interested was cast in.
  follows AS (
    SELECT h.user_id, h.object_id AS topic_id, max(h.voted_at) AS occurred_at
    FROM held h
    WHERE h.interested_in_topic
    GROUP BY h.user_id, h.object_id
  )
  -- Votes: any kind, either direction, on an entity -- except an Interested on a Topic, which is
  -- a follow (below), not a vote.
  SELECT h.user_id, 'vote'::text, h.object_id, h.object_id, false, max(h.voted_at)
  FROM held h
  WHERE NOT h.interested_in_topic
  GROUP BY h.user_id, h.object_id

  UNION ALL
  SELECT cr.user_id, 'comment', cr.target, cr.target, false,
         max(personalization.epoch_text_ts(e.created_at))
  FROM comment_roots cr
  JOIN public.entities e ON e.id = cr.comment_id
  GROUP BY cr.user_id, cr.target

  UNION ALL
  -- Debates: credited to each participant, for each thing debated.
  SELECT DISTINCT p.to_entity_id, 'debate', p.from_entity_id, dc.to_entity_id, false,
         personalization.epoch_text_ts(d.created_at)
  FROM public.relations p
  JOIN users u ON u.id = p.to_entity_id
  JOIN public.entities d ON d.id = p.from_entity_id
  JOIN public.relations dc ON dc.from_entity_id = p.from_entity_id
                          AND dc.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid
  WHERE p.type_id = '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid
    AND EXISTS (SELECT 1 FROM public.relations ty
                WHERE ty.from_entity_id = p.from_entity_id
                  AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                  AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)

  UNION ALL
  SELECT fo.user_id, 'follow', fo.topic_id, fo.topic_id, true, fo.occurred_at
  FROM follows fo

  UNION ALL
  SELECT x.user_id, x.kind, x.object_id, x.object_id, false, x.occurred_at
  FROM personalization.external_interest_signals x
  WHERE x.user_id = ANY(user_ids)
    AND EXISTS (SELECT 1 FROM users u WHERE u.id = x.user_id)
$$;
--> statement-breakpoint

UPDATE personalization.interest_signal_weights
   SET note = 'Follows the topic: a current Interested (vote_kind 3) on it (GEO-3158). The Following '
              'relation no longer counts. Fixed: does not decay.'
 WHERE kind = 'follow';
--> statement-breakpoint

UPDATE personalization.interest_signal_weights
   SET note = 'Superseded by GEO-3158: Interested on a topic is now a follow, read from user_votes. '
              'Nothing writes this kind; kept only because for_you_candidates and forYou/rank.ts name it.'
 WHERE kind = 'interested';
--> statement-breakpoint

UPDATE personalization.interest_signal_weights
   SET note = 'Any vote on an entity: up, down, agree, disagree, verify, dispute; Interested on a non-topic. '
              'Interested on a topic is a follow instead (GEO-3158).'
 WHERE kind = 'vote';
--> statement-breakpoint

-- CREATE OR REPLACE keeps the function's grants; restated as 0100 and 0105 do, in case it is ever
-- dropped and recreated.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
  END IF;
END
$$;
