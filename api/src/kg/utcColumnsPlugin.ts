/**
 * PostGraphile plugin that ensures *_utc columns are serialized as UTC.
 *
 * PostgreSQL's timestamptz stores values in UTC internally but returns them
 * in the session's timezone. This plugin intercepts the output for specific
 * UTC-normalized columns and ensures they're serialized with a Z suffix.
 */

// Column names that should be forced to UTC output
const UTC_COLUMNS = new Set(["time_utc", "datetime_utc"]);

export default function UtcColumnsPlugin(builder: any) {
	builder.hook(
		"GraphQLObjectType:fields:field",
		(field: any, build: any, context: any) => {
			const {
				scope: { pgFieldIntrospection },
			} = context;

			// Only process database columns
			if (!pgFieldIntrospection) {
				return field;
			}

			const columnName = pgFieldIntrospection.name;

			// Only process *_utc columns
			if (!UTC_COLUMNS.has(columnName)) {
				return field;
			}

			const originalResolve = field.resolve;

			return {
				...field,
				resolve: async (
					parent: any,
					args: any,
					ctx: any,
					info: any,
				) => {
					const result = originalResolve
						? await originalResolve(parent, args, ctx, info)
						: parent[info.fieldName];

					if (result instanceof Date) {
						return result.toISOString();
					}

					// Handle string timestamps that might have timezone offset
					if (typeof result === "string" && result.length > 0) {
						const date = new Date(result);
						if (!Number.isNaN(date.getTime())) {
							return date.toISOString();
						}
					}

					return result;
				},
			};
		},
	);
}
