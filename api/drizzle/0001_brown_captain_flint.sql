CREATE TABLE "members" (
	"address" text NOT NULL,
	"space_id" uuid NOT NULL,
	CONSTRAINT "members_address_space_id_pk" PRIMARY KEY("address","space_id")
);
