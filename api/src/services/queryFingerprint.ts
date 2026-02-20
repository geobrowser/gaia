function normalizeWhitespace(input: string): string {
	return input.replace(/\s+/g, " ").trim()
}

function fnv1a32(input: string): string {
	let hash = 0x811c9dc5
	for (let i = 0; i < input.length; i++) {
		hash ^= input.charCodeAt(i)
		hash = Math.imul(hash, 0x01000193)
	}
	return (hash >>> 0).toString(16).padStart(8, "0")
}

export function graphqlQueryFingerprint(query: string): string {
	return `gql:${fnv1a32(normalizeWhitespace(query))}`
}
