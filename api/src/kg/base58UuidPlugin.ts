import {Kind} from "graphql/language"
import type {GraphQLScalarType} from "graphql/type/definition"
import {fromBase58, uuidToBase58} from "../utils/uuid"

export function patchUuidScalar(uuidScalar: GraphQLScalarType): void {
	// Mutate the existing scalar instance so all references across the schema
	// automatically get the new behavior.
	uuidScalar.serialize = (value: unknown) => {
		if (typeof value !== "string") {
			throw new Error(`UUID cannot represent non-string value: ${typeof value}`)
		}
		return uuidToBase58(value)
	}
	uuidScalar.parseValue = (value: unknown) => {
		if (typeof value !== "string") {
			throw new Error(`UUID cannot represent non-string value: ${typeof value}`)
		}
		return fromBase58(value)
	}
	uuidScalar.parseLiteral = (ast) => {
		if (ast.kind !== Kind.STRING) {
			throw new Error("UUID can only parse string values")
		}
		return fromBase58(ast.value)
	}
	uuidScalar.description =
		"A universally unique identifier (UUID) as per RFC 4122. Accepts Base58-encoded input only; always serializes as Base58."
}

/**
 * Graphile Engine / PostGraphile plugin:
 * - accepts UUID inputs as Base58 only (rejects hex formats)
 * - serializes UUID outputs as Base58
 */
export default function Base58UuidPlugin(builder: any) {
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
