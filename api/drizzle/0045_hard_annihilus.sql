CREATE TYPE "public"."subspaceType" AS ENUM('verified', 'related');--> statement-breakpoint
CREATE TABLE "subspace_topics" (
	"space_id" uuid NOT NULL,
	"topic_id" uuid NOT NULL,
	CONSTRAINT "subspace_topics_space_id_topic_id_pk" PRIMARY KEY("space_id","topic_id")
);
--> statement-breakpoint
ALTER TABLE "subspaces" ADD COLUMN "type" "subspaceType" DEFAULT 'verified' NOT NULL;--> statement-breakpoint
ALTER TABLE "subspaces" DROP CONSTRAINT "subspaces_parent_space_id_child_space_id_pk";--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_parent_space_id_child_space_id_type_pk" PRIMARY KEY("parent_space_id","child_space_id","type");--> statement-breakpoint
CREATE INDEX "subspace_topics_space_id_idx" ON "subspace_topics" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "subspace_topics_topic_id_idx" ON "subspace_topics" USING btree ("topic_id");
