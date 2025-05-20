ALTER TABLE "relations" RENAME COLUMN "index" TO "position";--> statement-breakpoint
ALTER TABLE "relations" ADD COLUMN "entity_id" text NOT NULL;--> statement-breakpoint
ALTER TABLE "relations" ADD COLUMN "from_space_id" text;--> statement-breakpoint
ALTER TABLE "relations" ADD COLUMN "from_version_id" text;--> statement-breakpoint
ALTER TABLE "relations" ADD COLUMN "to_version_id" text;--> statement-breakpoint
ALTER TABLE "relations" ADD COLUMN "verified" boolean;