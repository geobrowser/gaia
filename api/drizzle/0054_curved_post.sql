CREATE TABLE "rate_limit_overrides" (
	"ip_range" "cidr" PRIMARY KEY NOT NULL,
	"requests_per_min" integer NOT NULL CHECK ("requests_per_min" >= 0),
	"description" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX "idx_rate_limit_overrides_ip_range" ON "rate_limit_overrides" USING gist ("ip_range" inet_ops);