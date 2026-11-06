-- Minimal tables the group B feed and primer reads touch, for gen_golden_feeds.py. Columns and types follow the api drizzle
-- migrations (0073, 0074, 0084, 0096, 0098, 0103); only what the extracted functions read is kept.
CREATE TABLE public.entities (id uuid PRIMARY KEY, created_at text NOT NULL);
CREATE TABLE public.spaces (id uuid PRIMARY KEY, type text NOT NULL, topic_id uuid);
CREATE TABLE public.relations (id uuid PRIMARY KEY, type_id uuid NOT NULL, from_entity_id uuid NOT NULL, to_entity_id uuid NOT NULL,
  position text, space_id uuid NOT NULL);
CREATE TABLE public."values" (id text PRIMARY KEY, property_id uuid NOT NULL, entity_id uuid NOT NULL, space_id uuid NOT NULL, text text);
CREATE TABLE public.entity_ranking_scores (entity_id uuid PRIMARY KEY, ranking_score numeric NOT NULL);
CREATE TABLE public.entity_feed_blocklist (entity_id uuid PRIMARY KEY);
CREATE TABLE public.entity_type_exclusions (type_id uuid PRIMARY KEY);
CREATE TABLE public.entity_type_ranking (type_id uuid NOT NULL, entity_id uuid NOT NULL, ranking_score numeric NOT NULL,
  PRIMARY KEY (type_id, entity_id));
CREATE TABLE public.entity_topic_ranking (topic_id uuid NOT NULL, type_id uuid NOT NULL, entity_id uuid NOT NULL,
  ranking_score numeric NOT NULL, created_epoch bigint, PRIMARY KEY (topic_id, type_id, entity_id));
CREATE TABLE public.user_votes (user_id uuid NOT NULL, object_id uuid NOT NULL, vote_kind smallint NOT NULL DEFAULT 0);
CREATE TABLE public.primer_claim_stats (claim_id uuid PRIMARY KEY, space_ids uuid[] NOT NULL, topic_ids uuid[] NOT NULL,
  voters integer NOT NULL, weighted_voters double precision NOT NULL, weighted_agree double precision NOT NULL,
  agree_share double precision NOT NULL, information double precision NOT NULL, score double precision NOT NULL,
  contested boolean NOT NULL, engagement double precision NOT NULL, name_length integer NOT NULL,
  computed_at timestamptz NOT NULL);
CREATE TABLE public.anchor_claim_sets (id serial PRIMARY KEY, effective_from timestamptz NOT NULL, claim_count integer NOT NULL,
  params jsonb NOT NULL, created_at timestamptz DEFAULT now() NOT NULL);
CREATE TABLE public.anchor_claim_members (set_id integer NOT NULL, claim_id uuid NOT NULL, rank integer NOT NULL,
  score double precision NOT NULL, weighted_voters double precision NOT NULL, agree_share double precision NOT NULL,
  PRIMARY KEY (set_id, claim_id));
