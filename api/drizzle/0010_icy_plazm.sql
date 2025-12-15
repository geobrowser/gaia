CREATE TABLE "global_scores" (
	"entity_id" uuid PRIMARY KEY NOT NULL,
	"score" numeric NOT NULL,
	"updated_at" timestamp with time zone NOT NULL
);
--> statement-breakpoint
CREATE TABLE "local_scores" (
	"entity_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"score" numeric NOT NULL,
	"updated_at" timestamp with time zone NOT NULL,
	CONSTRAINT "local_scores_entity_id_space_id_pk" PRIMARY KEY("entity_id","space_id")
);
--> statement-breakpoint
CREATE TABLE "space_scores" (
	"space_id" uuid PRIMARY KEY NOT NULL,
	"score" numeric NOT NULL,
	"updated_at" timestamp with time zone NOT NULL
);
--> statement-breakpoint
CREATE INDEX "idx_local_scores_space_id" ON "local_scores" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "idx_local_scores_entity_id" ON "local_scores" USING btree ("entity_id");