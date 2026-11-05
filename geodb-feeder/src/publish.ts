/**
 * Keeps the space map where the api can read it: always on the volume, and (in cluster) in the
 * ConfigMap the api pods mount. The ConfigMap is written through the Kubernetes API with the pod's own
 * service account; a failed write is retried until it lands, and never stops the feeder.
 */
import {readFileSync} from "node:fs"
import {request} from "node:https"
import {log, errorFields} from "./log.js"
import type {Metrics} from "./metrics.js"
import {writeFileDurable} from "./state.js"

const SA = "/var/run/secrets/kubernetes.io/serviceaccount"
/** The ConfigMap key the gaia Space map is published under; system Spaces use their own (systemSpaces.ts). */
export const SPACE_MAP_KEY = "space-map.json"

export type ConfigMapData = Record<string, string>

export function kubeConfigMapPut(name: string): ((data: ConfigMapData) => Promise<void>) | null {
	const host = process.env.KUBERNETES_SERVICE_HOST
	if (!name || !host) return null
	const port = process.env.KUBERNETES_SERVICE_PORT ?? "443"
	const call = (method: string, path: string, body: unknown, contentType: string) =>
		new Promise<{status: number; text: string}>((resolve, reject) => {
			const payload = JSON.stringify(body)
			const req = request(
				{
					host,
					port,
					path,
					method,
					ca: readFileSync(`${SA}/ca.crt`),
					headers: {
						authorization: `Bearer ${readFileSync(`${SA}/token`, "utf8").trim()}`,
						"content-type": contentType,
						"content-length": Buffer.byteLength(payload),
					},
					timeout: 15_000,
				},
				(res) => {
					let text = ""
					res.on("data", (d) => {
						text += d
					})
					res.on("end", () => resolve({status: res.statusCode ?? 0, text}))
				},
			)
			req.on("error", reject)
			req.on("timeout", () => req.destroy(new Error("kube api timeout")))
			req.end(payload)
		})
	return async (data) => {
		const namespace = readFileSync(`${SA}/namespace`, "utf8").trim()
		const base = `/api/v1/namespaces/${namespace}/configmaps`
		const patch = await call("PATCH", `${base}/${name}`, {data}, "application/merge-patch+json")
		if (patch.status === 200) return
		if (patch.status !== 404) throw new Error(`configmap patch: HTTP ${patch.status} ${patch.text.slice(0, 200)}`)
		const created = await call("POST", base, {metadata: {name}, data}, "application/json")
		if (created.status !== 201 && created.status !== 409) {
			throw new Error(`configmap create: HTTP ${created.status} ${created.text.slice(0, 200)}`)
		}
		if (created.status === 409) throw new Error("configmap appeared while creating; retrying as a patch")
	}
}

export class SpaceMapPublisher {
	private latest: string | null = null
	private dirty = false
	private inflight = false
	private timer: ReturnType<typeof setInterval> | null = null

	constructor(
		private readonly path: string,
		private readonly put: ((data: ConfigMapData) => Promise<void>) | null,
		private readonly metrics: Metrics,
		private readonly key: string = SPACE_MAP_KEY,
	) {}

	/** Record the newest map: durable on the volume now, in the ConfigMap as soon as the API takes it. */
	publish(text: string): void {
		writeFileDurable(this.path, text)
		this.latest = text
		this.dirty = true
		void this.flush()
	}

	/** True while the newest map has not reached the ConfigMap yet (always false outside a cluster). */
	get pending(): boolean {
		return this.put !== null && this.dirty
	}

	start(retryMs = 30_000): void {
		this.timer = setInterval(() => void this.flush(), retryMs)
		this.timer.unref()
	}

	stop(): void {
		if (this.timer) clearInterval(this.timer)
	}

	async flush(): Promise<void> {
		if (!this.put || !this.dirty || this.inflight || this.latest === null) return
		const text = this.latest
		const data: ConfigMapData = {[this.key]: text}
		this.inflight = true
		try {
			await this.put(data)
			if (this.latest === text) this.dirty = false
			this.metrics.spaceMapPublish.inc({key: this.key, result: "ok"})
		} catch (e) {
			this.metrics.spaceMapPublish.inc({key: this.key, result: "error"})
			log("warn", "space map publish failed, will retry", {key: this.key, ...errorFields(e)})
		} finally {
			this.inflight = false
		}
	}
}
