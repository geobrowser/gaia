ALTER TABLE "proposal_versions" ALTER COLUMN "partial_percentage_support_threshold" SET NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_versions" ALTER COLUMN "universal_percentage_support_threshold" SET NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_versions" ALTER COLUMN "flat_support_threshold" SET NOT NULL;