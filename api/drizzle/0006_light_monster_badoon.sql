ALTER TABLE "properties" RENAME TO "values";--> statement-breakpoint
ALTER TABLE "values" RENAME COLUMN "attribute_id" TO "property_id";--> statement-breakpoint
ALTER TABLE "values" RENAME COLUMN "text_value" TO "value";--> statement-breakpoint
ALTER TABLE "values" DROP COLUMN "number_value";--> statement-breakpoint
ALTER TABLE "values" DROP COLUMN "boolean_value";--> statement-breakpoint
ALTER TABLE "values" DROP COLUMN "language_option";--> statement-breakpoint
ALTER TABLE "values" DROP COLUMN "value_type";