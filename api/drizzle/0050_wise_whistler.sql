ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceVerified';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceUnverified';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceRelated';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceUnrelated';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceTopicDeclared';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'SubspaceTopicRemoved';--> statement-breakpoint
CREATE TABLE "app_webhooks" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"app_name" text NOT NULL,
	"url" text NOT NULL,
	"secret" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "app_webhooks_app_name_unique" UNIQUE("app_name")
);
--> statement-breakpoint
CREATE TABLE "notification_deliveries" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"outbox_id" uuid NOT NULL,
	"webhook_id" uuid NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL,
	"attempts" smallint DEFAULT 0 NOT NULL,
	"last_error" text,
	"next_retry_at" timestamp with time zone DEFAULT now() NOT NULL,
	"delivered_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "notification_deliveries_outbox_id_webhook_id_unique" UNIQUE("outbox_id","webhook_id")
);
--> statement-breakpoint
CREATE TABLE "notification_outbox" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"idempotency_key" text NOT NULL,
	"event_type" text NOT NULL,
	"payload" jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "notification_outbox_idempotency_key_unique" UNIQUE("idempotency_key")
);
--> statement-breakpoint
ALTER TABLE "notification_deliveries" ADD CONSTRAINT "notification_deliveries_outbox_id_notification_outbox_id_fk" FOREIGN KEY ("outbox_id") REFERENCES "public"."notification_outbox"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "notification_deliveries" ADD CONSTRAINT "notification_deliveries_webhook_id_app_webhooks_id_fk" FOREIGN KEY ("webhook_id") REFERENCES "public"."app_webhooks"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "idx_deliveries_pending" ON "notification_deliveries" USING btree ("status","next_retry_at");--> statement-breakpoint
ALTER TABLE "spaces" ADD CONSTRAINT "spaces_topic_id_entities_id_fk" FOREIGN KEY ("topic_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "subspace_topics" ADD CONSTRAINT "subspace_topics_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "subspace_topics" ADD CONSTRAINT "subspace_topics_topic_id_entities_id_fk" FOREIGN KEY ("topic_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_parent_space_id_spaces_id_fk" FOREIGN KEY ("parent_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_child_space_id_spaces_id_fk" FOREIGN KEY ("child_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;