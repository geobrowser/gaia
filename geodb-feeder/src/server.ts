import {createServer, type Server} from "node:http"

export type Health = {ready: () => boolean}

/** /health/liveness, /health/readiness and /health/metrics, the paths the other gaia services use; metrics are also served at /metrics, which the shared ServiceMonitor scrapes. */
export function startServer(port: number, metrics: {render(): string}, health: Health): Server {
	const server = createServer((req, res) => {
		const url = req.url ?? ""
		if (url.startsWith("/health/liveness")) {
			res.writeHead(200).end("ok\n")
		} else if (url.startsWith("/health/readiness")) {
			const ok = health.ready()
			res.writeHead(ok ? 200 : 503).end(ok ? "ready\n" : "not ready\n")
		} else if (url.startsWith("/health/metrics") || url === "/metrics") {
			res.writeHead(200, {"content-type": "text/plain; version=0.0.4"}).end(metrics.render())
		} else {
			res.writeHead(404).end()
		}
	})
	server.listen(port)
	return server
}
