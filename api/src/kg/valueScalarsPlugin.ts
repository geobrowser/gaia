import type { GraphQLNamedType, GraphQLOutputType } from "graphql";
import { GraphQLScalarType, Kind } from "graphql";

// =============================================================================
// Scalar definitions (name and description only - instances created at runtime)
// =============================================================================

interface ScalarDef {
  name: string;
  description: string;
  /** The GraphQL type name that PostGraphile auto-generates for this column. Defaults to "String". */
  sourceTypeName?: string;
}

const SCALAR_FIELD_NAMES = [
  "point",
  "rect",
  "date",
  "datetime",
  "time",
  "bytes",
  "language",
  "decimal",
] as const;

type ScalarFieldName = (typeof SCALAR_FIELD_NAMES)[number];

function isScalarFieldName(value: unknown): value is ScalarFieldName {
  return (
    typeof value === "string" &&
    SCALAR_FIELD_NAMES.includes(value as ScalarFieldName)
  );
}

const SCALAR_DEFS: Record<ScalarFieldName, ScalarDef> = {
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
  // Numeric scalars (Postgres numeric → String to preserve arbitrary precision)
  decimal: {
    name: "DecimalString",
    description:
      'Arbitrary-precision decimal number represented as a string to avoid floating-point precision loss. Examples: "123.456789", "0.001", "99999999999999999".',
    sourceTypeName: "BigFloat",
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
 * Most fields are remapped from String. The `decimal` field is special:
 * PostGraphile maps Postgres `numeric` to GraphQL `BigFloat`, so we remap
 * from BigFloat → DecimalString and coerce the value to a string on serialize.
 *
 * IMPORTANT: Scalars are created using build.graphql to avoid duplicate
 * graphql module issues in CI environments.
 */
export default function ValueScalarsPlugin(builder: any) {
  // Store scalar instances created during build phase
  let scalarInstances: Record<ScalarFieldName, GraphQLScalarType> | null = null;

  // Register all scalar types in the build phase
  builder.hook("build", (build: any) => {
    const {
      graphql: { GraphQLScalarType, Kind },
    } = build;

    const instances = {} as Record<ScalarFieldName, GraphQLScalarType>;

    // Create scalars using PostGraphile's graphql instance
    for (const fieldName of SCALAR_FIELD_NAMES) {
      const def = SCALAR_DEFS[fieldName];
      const scalar = new GraphQLScalarType({
        name: def.name,
        description: def.description,
        serialize: (value: unknown) => {
          // Coerce to string to preserve precision (important for decimal/numeric)
          if (typeof value === "number" || typeof value === "bigint") {
            return String(value);
          }
          return value;
        },
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
      instances[fieldName] = scalar;
      build.addType(scalar, "ValueScalarsPlugin");
    }

    scalarInstances = instances;
    return build;
  });

  // Remap fields to use custom scalars
  builder.hook(
    "GraphQLObjectType:fields:field",
    (field: any, build: any, context: any) => {
      const {
        graphql: { GraphQLNonNull, getNamedType, isNonNullType },
      } = build;

      const fieldName: unknown = context.scope?.fieldName;

      if (!isScalarFieldName(fieldName) || !scalarInstances) {
        return field;
      }

      const def = SCALAR_DEFS[fieldName];
      const scalar = scalarInstances[fieldName];

      // Check if field's underlying type matches the expected source type.
      // Most fields come from String columns, but decimal comes from Postgres
      // numeric which PostGraphile maps to BigFloat.
      const expectedSourceType = def.sourceTypeName ?? "String";
      const namedType: GraphQLNamedType | undefined = getNamedType(field.type);
      if (namedType?.name !== expectedSourceType) {
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
    serialize: (value: unknown) => {
      if (typeof value === "number" || typeof value === "bigint") {
        return String(value);
      }
      return value;
    },
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
export const GeoPointScalar = createTestScalar(SCALAR_DEFS.point);
export const GeoRectScalar = createTestScalar(SCALAR_DEFS.rect);
export const DateStringScalar = createTestScalar(SCALAR_DEFS.date);
export const DateTimeStringScalar = createTestScalar(SCALAR_DEFS.datetime);
export const TimeStringScalar = createTestScalar(SCALAR_DEFS.time);
export const BytesScalar = createTestScalar(SCALAR_DEFS.bytes);
export const LanguageTagScalar = createTestScalar(SCALAR_DEFS.language);
export const DecimalStringScalar = createTestScalar(SCALAR_DEFS.decimal);
