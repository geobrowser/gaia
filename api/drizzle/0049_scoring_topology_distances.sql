CREATE TABLE "scoring_topology_distances" (
	"space_id" uuid PRIMARY KEY NOT NULL,
	"distance" integer NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
