ALTER TABLE "editors" DROP CONSTRAINT "editors_address_space_id_pk";--> statement-breakpoint
ALTER TABLE "members" DROP CONSTRAINT "members_address_space_id_pk";--> statement-breakpoint
ALTER TABLE "editors" ADD COLUMN "member_space_id" uuid NOT NULL;--> statement-breakpoint
ALTER TABLE "members" ADD COLUMN "member_space_id" uuid NOT NULL;--> statement-breakpoint
ALTER TABLE "editors" ADD CONSTRAINT "editors_member_space_id_space_id_pk" PRIMARY KEY("member_space_id","space_id");--> statement-breakpoint
ALTER TABLE "members" ADD CONSTRAINT "members_member_space_id_space_id_pk" PRIMARY KEY("member_space_id","space_id");--> statement-breakpoint
ALTER TABLE "editors" DROP COLUMN "address";--> statement-breakpoint
ALTER TABLE "members" DROP COLUMN "address";