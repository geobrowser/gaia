/** Structured logs: one JSON object per line on stdout. */
export type Fields = Record<string, unknown>

export function log(level: "debug" | "info" | "warn" | "error", msg: string, fields: Fields = {}): void {
	process.stdout.write(`${JSON.stringify({ts: new Date().toISOString(), level, msg, ...fields})}\n`)
}

export const errorFields = (e: unknown): Fields => ({
	error: e instanceof Error ? e.message : String(e),
	stack: e instanceof Error ? e.stack?.split("\n").slice(0, 4).join(" | ") : undefined,
})
