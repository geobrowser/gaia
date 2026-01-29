import {
	GraphQLNonNull,
	GraphQLScalarType,
	Kind,
	isNonNullType,
	getNamedType,
} from "graphql";
import type { GraphQLOutputType, GraphQLNamedType } from "graphql";

/**
 * Helper to create a string-based scalar with validation.
 */
function createStringScalar(name: string, description: string): GraphQLScalarType {
	return new GraphQLScalarType({
		name,
		description,
		serialize: (value: unknown) => value,
		parseValue: (value: unknown) => {
			if (typeof value !== "string") {
				throw new Error(`${name} must be a string`);
			}
			return value;
		},
		parseLiteral: (ast) => {
			if (ast.kind !== Kind.STRING) {
				throw new Error(`${name} must be a string`);
			}
			return ast.value;
		},
	});
}

// =============================================================================
// Geo scalars
// =============================================================================

export const GeoPointScalar = createStringScalar(
	"GeoPoint",
	'WGS84 geographic coordinate. Format: "lat,lon" or "lat,lon,alt" where lat is [-90,90], lon is [-180,180], and alt is meters above ellipsoid.',
);

export const GeoRectScalar = createStringScalar(
	"GeoRect",
	'WGS84 axis-aligned bounding box. Format: "min_lat,min_lon,max_lat,max_lon" (southwest corner, then northeast corner). Coordinates follow same bounds as GeoPoint.',
);

// =============================================================================
// Temporal scalars (string representations preserving original timezone)
// These differ from PostgreSQL's native Date/Time/Datetime types which lose timezone info.
// =============================================================================

export const DateStringScalar = createStringScalar(
	"DateString",
	'Calendar date without time or timezone. Format: "YYYY-MM-DD" (ISO 8601). Unlike the Date scalar, this preserves the original string representation.',
);

export const DateTimeStringScalar = createStringScalar(
	"DateTimeString",
	'Date and time with optional timezone. Format: ISO 8601 (e.g., "2024-01-15T14:30:00" or "2024-01-15T14:30:00+05:30"). Preserves original timezone offset.',
);

export const TimeStringScalar = createStringScalar(
	"TimeString",
	'Time of day without date. Format: "HH:MM:SS" or "HH:MM:SS.sss" (ISO 8601). May include timezone offset.',
);

// =============================================================================
// Data format scalars
// =============================================================================

export const BytesScalar = createStringScalar(
	"Bytes",
	"Base64-encoded binary data (RFC 4648).",
);

export const LanguageTagScalar = createStringScalar(
	"LanguageTag",
	'BCP 47 language tag (e.g., "en", "en-US", "zh-Hans-CN").',
);

// =============================================================================
// Field name to scalar mapping
// =============================================================================

const FIELD_SCALAR_MAP: Record<string, GraphQLScalarType> = {
	point: GeoPointScalar,
	rect: GeoRectScalar,
	date: DateStringScalar,
	datetime: DateTimeStringScalar,
	time: TimeStringScalar,
	bytes: BytesScalar,
	language: LanguageTagScalar,
};

const ALL_SCALARS = Object.values(FIELD_SCALAR_MAP);

// =============================================================================
// Helpers
// =============================================================================

/**
 * Check if a field's underlying type is String (handling nullable wrappers).
 */
function isStringType(fieldType: GraphQLOutputType): boolean {
	const namedType: GraphQLNamedType | undefined = getNamedType(fieldType);
	return namedType?.name === "String";
}

/**
 * Replace the underlying type while preserving nullability wrapper.
 */
function replaceType(
	fieldType: GraphQLOutputType,
	newScalar: GraphQLScalarType,
): GraphQLOutputType {
	if (isNonNullType(fieldType)) {
		return new GraphQLNonNull(newScalar);
	}
	return newScalar;
}

// =============================================================================
// Plugin
// =============================================================================

/**
 * PostGraphile plugin that registers custom scalars for Value fields
 * to make the schema self-documenting with format information.
 *
 * These scalars are semantically equivalent to String but carry metadata
 * about the expected format (geo coordinates, dates, base64, etc.).
 */
export default function ValueScalarsPlugin(builder: any) {
	// Register all scalar types in the build phase
	builder.hook("build", (build: any) => {
		for (const scalar of ALL_SCALARS) {
			build.addType(scalar, "ValueScalarsPlugin");
		}
		return build;
	});

	// Remap fields to use custom scalars
	builder.hook(
		"GraphQLObjectType:fields:field",
		(field: any, _build: any, context: any) => {
			const fieldName = context.scope?.fieldName;
			const scalar = FIELD_SCALAR_MAP[fieldName];

			if (scalar && isStringType(field.type)) {
				return {
					...field,
					type: replaceType(field.type, scalar),
				};
			}

			return field;
		},
	);
}
