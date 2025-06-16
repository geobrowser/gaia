import {type SQL, and, inArray, not, or, sql} from "drizzle-orm"
import {entities, values} from "../../services/storage/schema"

type TextFilter = {
	is?: string
	contains?: string
	startsWith?: string
	endsWith?: string
	exists?: boolean
	NOT?: TextFilter
}

type NumberFilter = {
	is?: number
	lessThan?: number
	lessThanOrEqual?: number
	greaterThan?: number
	greaterThanOrEqual?: number
	exists?: boolean
	NOT?: NumberFilter
}

type CheckboxFilter = {
	is?: boolean
	exists?: boolean
}

type PointFilter = {
	is?: [number, number]
	exists?: boolean
}

type IdFilter = {
	in?: string[]
}

type PropertyFilter = {
	property: string
	text?: TextFilter
	number?: NumberFilter
	checkbox?: CheckboxFilter
	point?: PointFilter
}

type RelationFilter = {
	typeId?: string
	fromEntityId?: string
	toEntityId?: string
	spaceId?: string
}

export type EntityFilter = {
	AND?: EntityFilter[]
	OR?: EntityFilter[]
	NOT?: EntityFilter
	id?: IdFilter
	value?: PropertyFilter
	fromRelation?: RelationFilter
	toRelation?: RelationFilter
}

function buildValueConditions(filter: PropertyFilter) {
	const conditions = []

	if (filter.text) {
		const f = filter.text
		if (f.is !== undefined) conditions.push(sql`values.value = ${f.is}`)
		if (f.contains !== undefined) conditions.push(sql`values.value LIKE ${`%${f.contains}%`}`)
		if (f.startsWith !== undefined) conditions.push(sql`values.value LIKE ${`${f.startsWith}%`}`)
		if (f.endsWith !== undefined) conditions.push(sql`values.value LIKE ${`%${f.endsWith}`}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
	}

	if (filter.number) {
		const f = filter.number
		// Use CASE to safely cast, returning NULL for non-numeric values
		const safeCasted = sql`CASE
        WHEN values.value ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'

        THEN values.value::numeric
        ELSE NULL
    END`

		if (f.is !== undefined) conditions.push(sql`${safeCasted} = ${f.is}`)
		if (f.lessThan !== undefined) conditions.push(sql`${safeCasted} < ${f.lessThan}`)
		if (f.lessThanOrEqual !== undefined) conditions.push(sql`${safeCasted} <= ${f.lessThanOrEqual}`)
		if (f.greaterThan !== undefined) conditions.push(sql`${safeCasted} > ${f.greaterThan}`)
		if (f.greaterThanOrEqual !== undefined) conditions.push(sql`${safeCasted} >= ${f.greaterThanOrEqual}`)

		if (f.exists !== undefined) {
			// For exists, check if the value exists AND is numeric
			const isNumeric = sql`values.value ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'`

			if (f.exists) {
				conditions.push(sql`(values.value IS NOT NULL AND ${isNumeric})`)
			} else {
				conditions.push(sql`(values.value IS NULL OR NOT ${isNumeric})`)
			}
		}
	}

	if (filter.checkbox) {
		const f = filter.checkbox
		if (f.is !== undefined) conditions.push(sql`values.value = ${f.is.toString()}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
	}

	if (filter.point) {
		const f = filter.point
		if (f.is !== undefined) conditions.push(sql`values.value = ${JSON.stringify(f.is)}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
	}

	return conditions
}

function buildValueWhere(filter: PropertyFilter, spaceId?: string | null) {
	const conditions = [sql`values.property_id = ${filter.property}`]

	// Add spaceId filtering if provided
	if (spaceId) {
		conditions.push(sql`values.space_id = ${spaceId}`)
	}

	if (filter.text) {
		const f = filter.text
		if (f.is !== undefined) conditions.push(sql`values.value = ${f.is}`)
		if (f.contains !== undefined) conditions.push(sql`values.value LIKE ${`%${f.contains}%`}`)
		if (f.startsWith !== undefined) conditions.push(sql`values.value LIKE ${`${f.startsWith}%`}`)
		if (f.endsWith !== undefined) conditions.push(sql`values.value LIKE ${`%${f.endsWith}`}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
		if (f.NOT) {
			const notConditions = buildValueConditions({property: filter.property, text: f.NOT})
			if (notConditions.length > 0) {
				conditions.push(not(sql.join(notConditions, sql` AND `)))
			}
		}
	}

	if (filter.number) {
		const f = filter.number
		// Use CASE to safely cast, returning NULL for non-numeric values
		const safeCasted = sql`CASE
        WHEN values.value ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'

        THEN values.value::numeric
        ELSE NULL
    END`

		if (f.is !== undefined) conditions.push(sql`${safeCasted} = ${f.is}`)
		if (f.lessThan !== undefined) conditions.push(sql`${safeCasted} < ${f.lessThan}`)
		if (f.lessThanOrEqual !== undefined) conditions.push(sql`${safeCasted} <= ${f.lessThanOrEqual}`)
		if (f.greaterThan !== undefined) conditions.push(sql`${safeCasted} > ${f.greaterThan}`)
		if (f.greaterThanOrEqual !== undefined) conditions.push(sql`${safeCasted} >= ${f.greaterThanOrEqual}`)

		if (f.exists !== undefined) {
			// For exists, check if the value exists AND is numeric
			const isNumeric = sql`values.value ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'`

			if (f.exists) {
				conditions.push(sql`(values.value IS NOT NULL AND ${isNumeric})`)
			} else {
				conditions.push(sql`(values.value IS NULL OR NOT ${isNumeric})`)
			}
		}

		if (f.NOT) {
			const notConditions = buildValueConditions({property: filter.property, number: f.NOT})
			if (notConditions.length > 0) {
				conditions.push(not(sql.join(notConditions, sql` AND `)))
			}
		}
	}

	if (filter.checkbox) {
		const f = filter.checkbox
		if (f.is !== undefined) conditions.push(sql`values.value = ${f.is.toString()}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
	}

	if (filter.point) {
		const f = filter.point
		if (f.is !== undefined) conditions.push(sql`values.value = ${JSON.stringify(f.is)}`)
		if (f.exists !== undefined) {
			conditions.push(f.exists ? sql`values.value IS NOT NULL` : sql`values.value IS NULL`)
		}
	}

	return sql.join(conditions, sql` AND `)
}

function buildRelationConditions(filter: RelationFilter, spaceId?: string | null) {
	const conditions = []

	if (filter.typeId !== undefined) {
		conditions.push(sql`type_id = ${filter.typeId}`)
	}
	if (filter.fromEntityId !== undefined) {
		conditions.push(sql`from_entity_id = ${filter.fromEntityId}`)
	}
	if (filter.toEntityId !== undefined) {
		conditions.push(sql`to_entity_id = ${filter.toEntityId}`)
	}
	if (filter.spaceId !== undefined) {
		conditions.push(sql`space_id = ${filter.spaceId}`)
	}

	// Add spaceId filtering if provided and not already specified in filter
	if (spaceId && filter.spaceId === undefined) {
		conditions.push(sql`space_id = ${spaceId}`)
	}

	return conditions.length > 0 ? sql.join(conditions, sql` AND `) : undefined
}

export function buildEntityWhere(filter: EntityFilter | null, spaceId?: string | null): SQL | undefined {
	if (!filter && !spaceId) return undefined

	const clauses = []

	// Add spaceId filtering if provided
	if (spaceId) {
		clauses.push(
			sql`(
				EXISTS (
					SELECT 1 FROM ${values}
					WHERE values.entity_id = entities.id
					AND values.space_id = ${spaceId}
				) OR EXISTS (
					SELECT 1 FROM relations
					WHERE relations.from_entity_id = entities.id
					AND relations.space_id = ${spaceId}
				)
			)`,
		)
	}

	if (filter?.id) {
		if (filter.id.in && filter.id.in.length > 0) {
			clauses.push(inArray(entities.id, filter.id.in))
		} else if (filter.id.in && filter.id.in.length === 0) {
			// Empty array should return no results
			clauses.push(sql`false`)
		}
	}
	if (filter?.AND) {
		clauses.push(and(...filter.AND.map((f) => buildEntityWhere(f, spaceId))))
	}
	if (filter?.OR) {
		clauses.push(or(...filter.OR.map((f) => buildEntityWhere(f, spaceId))))
	}
	if (filter?.NOT) {
		const notCondition = buildEntityWhere(filter.NOT, spaceId)
		if (notCondition) {
			clauses.push(not(notCondition))
		}
	}
	if (filter?.value) {
		// This checks: exists a value with this filter for the entity
		clauses.push(
			sql`EXISTS (
        SELECT 1 FROM ${values}
        WHERE values.entity_id = entities.id
        AND ${buildValueWhere(filter.value, spaceId)}
      )`,
		)
	}
	if (filter?.fromRelation) {
		// This checks: exists a relation where this entity is the fromEntity
		const relationConditions = buildRelationConditions(filter.fromRelation, spaceId)
		if (relationConditions) {
			clauses.push(
				sql`EXISTS (
          SELECT 1 FROM relations
          WHERE from_entity_id = entities.id AND ${relationConditions}
        )`,
			)
		} else {
			clauses.push(
				sql`EXISTS (
          SELECT 1 FROM relations
          WHERE from_entity_id = entities.id
        )`,
			)
		}
	}
	if (filter?.toRelation) {
		// This checks: exists a relation where this entity is the toEntity
		const relationConditions = buildRelationConditions(filter.toRelation, spaceId)
		if (relationConditions) {
			clauses.push(
				sql`EXISTS (
          SELECT 1 FROM relations
          WHERE to_entity_id = entities.id AND ${relationConditions}
        )`,
			)
		} else {
			clauses.push(
				sql`EXISTS (
          SELECT 1 FROM relations
          WHERE to_entity_id = entities.id
        )`,
			)
		}
	}

	return and(...clauses)
}
