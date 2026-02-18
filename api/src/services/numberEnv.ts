export function parsePositiveIntEnv(name: string, fallback: number): number {
	const raw = process.env[name]
	if (!raw) {
		return fallback
	}

	const parsed = Number.parseInt(raw, 10)
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return fallback
	}

	return parsed
}
