import { Kind } from "graphql/language"
import type { GraphQLScalarType } from "graphql/type/definition"
import { normalizeUuid } from "../utils/uuid"

export function patchUuidScalar(uuidScalar: GraphQLScalarType): void {
	// Mutate the existing scalar instance so all references across the schema
	// automatically get the new behavior.
	uuidScalar.serialize = (value: unknown) => normalizeUuid(String(value))
	uuidScalar.parseValue = (value: unknown) => {
		if (typeof value !== "string") {
			throw new Error(`UUID cannot represent non-string value: ${String(value)}`)
		}
		return normalizeUuid(value)
	}
	uuidScalar.parseLiteral = (ast) => {
		if (ast.kind !== Kind.STRING) {
			throw new Error("UUID can only parse string values")
		}
		return normalizeUuid(ast.value)
	}
	// Update the description to clarify undashed serialization and flexible input
	uuidScalar.description =
		"A universally unique identifier (UUID) as per RFC 4122. Accepts dashed or undashed input; always serializes without dashes."
}

/**
 * Graphile Engine / PostGraphile plugin:
 * - accepts UUID inputs with and without dashes
 * - serializes UUID outputs without dashes
 */
export default function UndashedUuidPlugin(builder: any) {
	builder.hook("GraphQLSchema", (schema: any, build: any) => {
		// PostGraphile v4 defaults to 'UUID', but the legacy option uses 'Uuid'
		const candidates = ["UUID", "Uuid"]
		for (const name of candidates) {
			const t = build.getTypeByName?.(name)
			if (t && typeof t === "object" && t.constructor?.name === "GraphQLScalarType") {
				patchUuidScalar(t as GraphQLScalarType)
			}
		}
		return schema
	})
}


