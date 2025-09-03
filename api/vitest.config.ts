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
		env: {
			DATABASE_URL: process.env.DATABASE_URL || "postgresql://localhost:5432/gaia",
			IPFS_KEY: "test-key",
			IPFS_GATEWAY_WRITE: "https://ipfs.io",
			RPC_ENDPOINT: "https://rpc.testnet.example.com",
			CHAIN_ID: "80451",
			DEPLOYER_PK: "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
		},
		coverage: {
			provider: "v8",
			reporter: ["text", "json", "html"],
			include: ["src/**/*.{js,ts}"],
			exclude: ["src/**/*.{test,spec}.{js,ts}", "src/generated/**"],
		},
	},
})
