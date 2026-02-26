CREATE TABLE "atlas_checkpoints" (
	"indexer_id" text PRIMARY KEY NOT NULL,
	"cursor" text NOT NULL,
	"block_number" bigint NOT NULL,
	"graph_state_version" smallint NOT NULL,
	"runtime_compatibility_marker" text NOT NULL,
	"root_space_id" text NOT NULL,
	"schema_version" smallint NOT NULL,
	"graph_state_blob" jsonb NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
