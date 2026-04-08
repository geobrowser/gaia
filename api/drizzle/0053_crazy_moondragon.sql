ALTER TYPE "public"."proposalActionType" ADD VALUE 'SetTopic';--> statement-breakpoint
ALTER TYPE "public"."proposalActionType" ADD VALUE 'UnsetTopic';--> statement-breakpoint
CREATE INDEX "idx_local_scores_space_score" ON "local_scores" USING btree ("space_id","score");