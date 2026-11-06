-- Minimal tables the group B reads touch, for the golden-grid generator (gen_golden.py). Column names and types follow the
-- api drizzle migrations; only the columns the extracted functions read are kept.
CREATE SCHEMA IF NOT EXISTS trending;
CREATE TABLE public.entities (id uuid PRIMARY KEY, created_at text NOT NULL);
CREATE TABLE public.spaces (id uuid PRIMARY KEY, type text NOT NULL, topic_id uuid);
CREATE TABLE public.relations (id uuid PRIMARY KEY, type_id uuid NOT NULL, from_entity_id uuid NOT NULL, to_entity_id uuid NOT NULL,
  position text, space_id uuid NOT NULL);
CREATE TABLE public."values" (id text PRIMARY KEY, property_id uuid NOT NULL, entity_id uuid NOT NULL, space_id uuid NOT NULL,
  text text, datetime_utc timestamptz, datetime text, date text);
CREATE TABLE trending.agenda_listings (agenda_id uuid NOT NULL, space_id uuid NOT NULL, entity_id uuid NOT NULL, listed_on date NOT NULL,
  source text NOT NULL, PRIMARY KEY (agenda_id, space_id, entity_id, source));
CREATE TABLE public.space_topic_suggestions (space_id uuid NOT NULL, topic_id uuid NOT NULL, name_key text NOT NULL, links integer NOT NULL,
  rank integer NOT NULL, PRIMARY KEY (space_id, topic_id));
