CREATE TABLE "relations" (
	"id" text PRIMARY KEY NOT NULL,
	"type_id" text NOT NULL,
	"from_entity_id" text NOT NULL,
	"to_entity_id" text NOT NULL,
	"to_space_id" text NOT NULL,
	"index" text
);
