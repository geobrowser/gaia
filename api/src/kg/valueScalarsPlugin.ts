import { GraphQLScalarType, Kind } from "graphql";
import type { GraphQLOutputType, GraphQLNamedType } from "graphql";

// =============================================================================
// Scalar definitions (name and description only - instances created at runtime)
// =============================================================================

interface ScalarDef {
	name: string;
	description: string;
}

const SCALAR_DEFS: Record<string, ScalarDef> = {
	// Geo scalars
	point: {
		name: "GeoPoint",
		description:
			'WGS84 geographic coordinate. Format: "lat,lon" or "lat,lon,alt" where lat is [-90,90], lon is [-180,180], and alt is meters above ellipsoid.',
	},
	rect: {
		name: "GeoRect",
		description:
			'WGS84 axis-aligned bounding box. Format: "min_lat,min_lon,max_lat,max_lon" (southwest corner, then northeast corner). Coordinates follow same bounds as GeoPoint.',
	},
	// Temporal scalars (string representations preserving original timezone)
	date: {
		name: "DateString",
		description:
			'Calendar date without time or timezone. Format: "YYYY-MM-DD" (ISO 8601). Unlike the Date scalar, this preserves the original string representation.',
	},
	datetime: {
		name: "DateTimeString",
		description:
			'Date and time with optional timezone. Format: ISO 8601 (e.g., "2024-01-15T14:30:00" or "2024-01-15T14:30:00+05:30"). Preserves original timezone offset.',
	},
	time: {
		name: "TimeString",
		description:
			'Time of day without date. Format: "HH:MM:SS" or "HH:MM:SS.sss" (ISO 8601). May include timezone offset.',
	},
	// Data format scalars
	bytes: {
		name: "Bytes",
		description: "Base64-encoded binary data (RFC 4648).",
	},
	language: {
		name: "LanguageTag",
		description: 'BCP 47 language tag (e.g., "en", "en-US", "zh-Hans-CN").',
	},
};

// =============================================================================
// Plugin
// =============================================================================

/**
 * PostGraphile plugin that registers custom scalars for Value fields
 * to make the schema self-documenting with format information.
 *
 * These scalars are semantically equivalent to String but carry metadata
 * about the expected format (geo coordinates, dates, base64, etc.).
 *
 * IMPORTANT: Scalars are created using build.graphql to avoid duplicate
 * graphql module issues in CI environments.
 */
export default function ValueScalarsPlugin(builder: any) {
	// Store scalar instances created during build phase
	let scalarInstances: Record<string, GraphQLScalarType> = {};

	// Register all scalar types in the build phase
	builder.hook("build", (build: any) => {
		const {
			graphql: { GraphQLScalarType, Kind },
		} = build;

		// Create scalars using PostGraphile's graphql instance
		for (const [fieldName, def] of Object.entries(SCALAR_DEFS)) {
			const scalar = new GraphQLScalarType({
				name: def.name,
				description: def.description,
				serialize: (value: unknown) => value,
				parseValue: (value: unknown) => {
					if (typeof value !== "string") {
						throw new Error(`${def.name} must be a string`);
					}
					return value;
				},
				parseLiteral: (ast: any) => {
					if (ast.kind !== Kind.STRING) {
						throw new Error(`${def.name} must be a string`);
					}
					return ast.value;
				},
			});
			scalarInstances[fieldName] = scalar;
			build.addType(scalar, "ValueScalarsPlugin");
		}

		return build;
	});

	// Remap fields to use custom scalars
	builder.hook(
		"GraphQLObjectType:fields:field",
		(field: any, build: any, context: any) => {
			const {
				graphql: { GraphQLNonNull, getNamedType, isNonNullType },
			} = build;

			const fieldName = context.scope?.fieldName;
			const scalar = scalarInstances[fieldName];

			if (!scalar) {
				return field;
			}

			// Check if field's underlying type is String
			const namedType: GraphQLNamedType | undefined = getNamedType(field.type);
			if (namedType?.name !== "String") {
				return field;
			}

			// Replace type while preserving nullability
			const newType: GraphQLOutputType = isNonNullType(field.type)
				? new GraphQLNonNull(scalar)
				: scalar;

			return {
				...field,
				type: newType,
			};
		},
	);
}

// =============================================================================
// Exports for unit testing scalar behavior (not used by plugin)
// =============================================================================

function createTestScalar(def: ScalarDef): GraphQLScalarType {
	return new GraphQLScalarType({
		name: def.name,
		description: def.description,
		serialize: (value: unknown) => value,
		parseValue: (value: unknown) => {
			if (typeof value !== "string") {
				throw new Error(`${def.name} must be a string`);
			}
			return value;
		},
		parseLiteral: (ast) => {
			if (ast.kind !== Kind.STRING) {
				throw new Error(`${def.name} must be a string`);
			}
			return ast.value;
		},
	});
}

// Test-only scalar instances (use local graphql module)
export const GeoPointScalar = createTestScalar(SCALAR_DEFS.point!);
export const GeoRectScalar = createTestScalar(SCALAR_DEFS.rect!);
export const DateStringScalar = createTestScalar(SCALAR_DEFS.date!);
export const DateTimeStringScalar = createTestScalar(SCALAR_DEFS.datetime!);
export const TimeStringScalar = createTestScalar(SCALAR_DEFS.time!);
export const BytesScalar = createTestScalar(SCALAR_DEFS.bytes!);
export const LanguageTagScalar = createTestScalar(SCALAR_DEFS.language!);
