import tsconfigPaths from "vite-tsconfig-paths"
/// <reference types="vitest" />
import {defineConfig} from "vitest/config"

export default defineConfig({
	plugins: [tsconfigPaths()],
	test: {
		globals: true,
		environment: "node",
		include: ["src/**/*.{test,spec}.{js,mjs,cjs,ts,mts,cts,jsx,tsx}"],
		exclude: ["node_modules", "dist", ".git", ".cache"],
		setupFiles: ["./src/test-setup.ts"],
		coverage: {
			provider: "v8",
			reporter: ["text", "json", "html"],
			include: ["src/**/*.{js,ts}"],
			exclude: ["src/**/*.{test,spec}.{js,ts}", "src/generated/**"],
		},
	},
})
