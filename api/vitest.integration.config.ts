import tsconfigPaths from "vite-tsconfig-paths"
import {defineConfig} from "vitest/config"

/**
 * Vitest config for integration tests that require real external services
 * (Postgres, Valkey). These are excluded from the default vitest.config.ts
 * and run in dedicated CI workflows with service containers.
 */
export default defineConfig({
	plugins: [tsconfigPaths()],
	test: {
		globals: true,
		environment: "node",
		include: ["src/middleware/__tests__/rateLimit-integration.test.ts"],
	},
})
