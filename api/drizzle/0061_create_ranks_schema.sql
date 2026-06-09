-- ranking-indexer working schema (GEO ranking aggregation).
--
-- These tables are the ranking-indexer's PRIVATE working cache. They live in a
-- dedicated `ranks` schema (NOT `public`) on purpose: PostGraphile introspects
-- only `public`, so nothing here reaches the GraphQL API. Every value is
-- derivable from the `knowledge.edits` stream and can be rebuilt by replay —
-- this is a cache, not a source of truth. The indexer joins the public
-- `members`/`editors`/`spaces`/`entities` tables cross-schema during
-- aggregation, which is why these sit in the same database.
--
-- NOTE: some column shapes are inferred from the design doc (prose-level) and
-- may be refined once the decode path is built — e.g. the dedup "update
-- markers" on `rankings`, date storage, and the restriction representation.
-- Because the schema is a rebuildable cache, those are low-risk to adjust.

CREATE SCHEMA IF NOT EXISTS ranks;
--> statement-breakpoint

-- One row per Ranking Block entity.
CREATE TABLE IF NOT EXISTS ranks.ranking_blocks (
	"id" uuid PRIMARY KEY NOT NULL,                -- Ranking Block entity id
	"space_id" uuid NOT NULL,                      -- space the block lives in
	"name" text,
	"filter" text,                                 -- query-data-block filter string (optional)
	"start_date" timestamp with time zone,         -- optional submission window (inclusive)
	"end_date" timestamp with time zone,
	"restriction_id" uuid,                         -- aggregation restriction value entity; NULL => default "Members and editors"
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint

-- One row per Rank submission entity.
CREATE TABLE IF NOT EXISTS ranks.rankings (
	"id" uuid PRIMARY KEY NOT NULL,                -- Rank entity id
	"block_id" uuid,                               -- nullable until the RANK_BLOCK link arrives (partial-state model)
	"space_id" uuid NOT NULL,                      -- submitter's personal space
	"author_address" text,                         -- submitter address resolved from the personal space
	"rank_type" text,                              -- 'ORDINAL' | 'WEIGHTED'
	"submitted_at" timestamp with time zone,       -- submission timestamp used for the window check
	"updated_at_block" bigint DEFAULT 0 NOT NULL,  -- update markers for dedup: most-recently-updated rank per (block, space) wins
	"update_index" bigint DEFAULT 0 NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "rankings_block_id_idx" ON ranks.rankings ("block_id");
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "rankings_block_space_idx" ON ranks.rankings ("block_id","space_id");
--> statement-breakpoint

-- Decoded items of each submission, keyed on (ranking, entity, space) so a user
-- can rank competing perspectives of the same subject.
CREATE TABLE IF NOT EXISTS ranks.ranking_items (
	"ranking_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,                     -- ranked entity
	"space_id" uuid NOT NULL,                      -- to_space_id (perspective of the ranked entity)
	"position" text,                               -- fractional index (ordinal ordering)
	"weight" double precision,                     -- weighted value (NULL for ordinal ranks)
	CONSTRAINT "ranking_items_pkey" PRIMARY KEY ("ranking_id","entity_id","space_id")
);
--> statement-breakpoint

-- Computed aggregate, keyed on (block, entity, space).
CREATE TABLE IF NOT EXISTS ranks.ranking_scores (
	"block_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"score" double precision NOT NULL,             -- summed contribution across eligible submissions
	"position" integer NOT NULL,                   -- final integer rank within the block
	CONSTRAINT "ranking_scores_pkey" PRIMARY KEY ("block_id","entity_id","space_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "ranking_scores_block_position_idx" ON ranks.ranking_scores ("block_id","position");
