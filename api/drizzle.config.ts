import {defineConfig} from "drizzle-kit"

// Migrations require direct DB connection (not through connection pooler)
// because they use prepared statements, SET commands, and advisory locks
const databaseUrl = process.env.DATABASE_URL_DIRECT || process.env.DATABASE_URL

if (!databaseUrl) {
	throw new Error("DATABASE_URL_DIRECT or DATABASE_URL must be set")
}

export default defineConfig({
	dialect: "postgresql",
	schema: "./src/services/storage/schema.ts",
	dbCredentials: {
		url: databaseUrl,
	},
	casing: "snake_case",
})
