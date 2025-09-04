#!/usr/bin/env bun

import {$} from "bun"
import {Pool} from "pg"

async function setupTestDatabase() {
	const databaseUrl = process.env.DATABASE_URL

	if (!databaseUrl) {
		console.error("❌ DATABASE_URL environment variable is not set")
		process.exit(1)
	}

	console.log("🔧 Setting up test database...")
	console.log(`📡 Database URL: ${databaseUrl.replace(/:[^:@]*@/, ":****@")}`)

	try {
		// Test database connection
		console.log("🔍 Testing database connection...")
		const pool = new Pool({connectionString: databaseUrl})

		await pool.query("SELECT NOW()")
		console.log("✅ Database connection successful")

		await pool.end()

		// Run migrations using drizzle-kit
		console.log("📋 Running database migrations...")
		const result = await $`bun drizzle-kit push`.env({
			DATABASE_URL: databaseUrl,
		})

		if (result.exitCode === 0) {
			console.log("✅ Database migrations applied successfully")
		} else {
			console.error("❌ Failed to apply database migrations")
			console.error(result.stderr.toString())
			process.exit(1)
		}

		console.log("🔧 Running SQL functions script...")
		const fnResult = await $`./scripts/migrate-custom.sh`.env({
			DATABASE_URL: databaseUrl,
		})

		if (fnResult.exitCode === 0) {
			console.log("✅ SQL functions applied successfully")
		} else {
			console.error("❌ Failed to apply SQL functions")
			console.error(fnResult.stderr.toString())
			process.exit(1)
		}

		// Verify schema by checking if tables exist
		console.log("🔍 Verifying schema...")
		const verifyPool = new Pool({connectionString: databaseUrl})

		const tables = await verifyPool.query(`
      SELECT table_name
      FROM information_schema.tables
      WHERE table_schema = 'public'
      ORDER BY table_name
    `)

		console.log("📊 Tables created:")
		tables.rows.forEach((row) => {
			console.log(`  - ${row.table_name}`)
		})

		if (tables.rows.length === 0) {
			console.warn("⚠️  No tables found - schema might not have been applied")
			process.exit(1)
		}

		await verifyPool.end()

		console.log("🎉 Test database setup completed successfully!")
	} catch (error) {
		console.error("❌ Database setup failed:")
		console.error(error)
		process.exit(1)
	}
}

// Run the setup if this script is executed directly
if (import.meta.main) {
	setupTestDatabase()
}

export {setupTestDatabase}
