import { GraphQLScalarType, Kind } from "graphql";

/**
 * GeoPoint scalar for WGS84 coordinates.
 * Format: "lat,lon" or "lat,lon,alt"
 */
export const GeoPointScalar = new GraphQLScalarType({
	name: "GeoPoint",
	description:
		'WGS84 geographic coordinate. Format: "lat,lon" or "lat,lon,alt" where lat is [-90,90], lon is [-180,180], and alt is meters above ellipsoid.',
	serialize: (value: unknown) => value,
	parseValue: (value: unknown) => {
		if (typeof value !== "string") {
			throw new Error("GeoPoint must be a string");
		}
		return value;
	},
	parseLiteral: (ast) => {
		if (ast.kind !== Kind.STRING) {
			throw new Error("GeoPoint must be a string");
		}
		return ast.value;
	},
});

/**
 * GeoRect scalar for WGS84 bounding boxes.
 * Format: "min_lat,min_lon,max_lat,max_lon"
 */
export const GeoRectScalar = new GraphQLScalarType({
	name: "GeoRect",
	description:
		'WGS84 axis-aligned bounding box. Format: "min_lat,min_lon,max_lat,max_lon" (southwest corner, then northeast corner). Coordinates follow same bounds as GeoPoint.',
	serialize: (value: unknown) => value,
	parseValue: (value: unknown) => {
		if (typeof value !== "string") {
			throw new Error("GeoRect must be a string");
		}
		return value;
	},
	parseLiteral: (ast) => {
		if (ast.kind !== Kind.STRING) {
			throw new Error("GeoRect must be a string");
		}
		return ast.value;
	},
});

/**
 * PostGraphile plugin that registers GeoPoint and GeoRect scalars
 * and remaps `point` and `rect` fields to use them.
 */
export default function GeoScalarPlugin(builder: any) {
	// Register the scalar types
	builder.hook("build", (build: any) => {
		build.addType(GeoPointScalar);
		build.addType(GeoRectScalar);
		return build;
	});

	// Remap point and rect fields to use custom scalars
	builder.hook(
		"GraphQLObjectType:fields:field",
		(field: any, build: any, context: any) => {
			const { fieldName } = context;

			if (fieldName === "point" && field.type?.name === "String") {
				return {
					...field,
					type: GeoPointScalar,
				};
			}

			if (fieldName === "rect" && field.type?.name === "String") {
				return {
					...field,
					type: GeoRectScalar,
				};
			}

			return field;
		},
	);
}
