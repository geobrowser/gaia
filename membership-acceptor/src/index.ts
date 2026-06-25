/**
 * Membership Acceptor — entry point.
 *
 * A long-running HTTP server that receives notification webhooks from the Geo
 * notification service. This is the real-time successor to the proposal-executor
 * cron's membership-accept path.
 *
 * Milestone 1 (this file): stand up the server, verify webhook signatures, and
 * log deliveries. Detection (M2) and on-chain voting (M3) build on top.
 */

import {parseConfig} from "./config.js"
import {createApp} from "./server.js"
import {flush, log} from "./telemetry.js"

async function main() {
	let config: ReturnType<typeof parseConfig>
	try {
		config = parseConfig()
	} catch (err) {
		// Misconfiguration — crash loudly so the deployment is visibly broken.
		log.error("configuration error — refusing to start", {error: err})
		// Flush buffered telemetry before exiting, or the startup-failure event is
		// lost (same reason the graceful-shutdown path flushes before exit).
		await flush()
		process.exit(1)
	}

	const app = createApp(config)

	const server = Bun.serve({
		port: config.port,
		fetch: app,
	})

	log.info("membership-acceptor listening", {
		port: server.port,
		webhook_path: "/webhooks/geo",
		health_path: "/health",
	})

	// Graceful shutdown: stop accepting connections, flush telemetry, then exit.
	let shuttingDown = false
	const shutdown = async (signal: string) => {
		if (shuttingDown) return
		shuttingDown = true
		log.info("shutting down", {signal})
		await server.stop()
		await flush()
		process.exit(0)
	}

	process.on("SIGTERM", () => void shutdown("SIGTERM"))
	process.on("SIGINT", () => void shutdown("SIGINT"))
}

main()
