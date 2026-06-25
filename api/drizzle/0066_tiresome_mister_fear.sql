ALTER TABLE "app_webhooks" ADD COLUMN IF NOT EXISTS "notification_types" text[];--> statement-breakpoint
ALTER TABLE "app_webhooks" ADD COLUMN IF NOT EXISTS "space_ids" uuid[];