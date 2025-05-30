ALTER TABLE "entities" ALTER COLUMN "id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "properties" ALTER COLUMN "id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "entity_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "type_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "from_entity_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "from_space_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "from_version_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "to_entity_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "to_space_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "to_version_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "relations" ALTER COLUMN "space_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "property_id" SET DATA TYPE uuid USING id::uuid;--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "entity_id" SET DATA TYPE uuid USING id::uuid;
