CREATE TABLE "api_keys" (
	"key" text PRIMARY KEY NOT NULL,
	"client_name" text NOT NULL,
	"requests_per_min" integer,
	"enabled" boolean DEFAULT true NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
