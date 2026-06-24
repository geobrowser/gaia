ALTER TABLE "app_webhooks" ADD COLUMN "notification_types" text[];--> statement-breakpoint
ALTER TABLE "app_webhooks" ADD COLUMN "space_ids" uuid[];