import {and, eq, isNull, like, not, or, sql, type SQL} from "drizzle-orm"
import {entities, values} from "../services/storage/schema"

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
	value?: PropertyFilter
	fromRelation?: RelationFilter
	toRelation?: RelationFilter
}

function buildValueWhere(filter: PropertyFilter) {
	const conditions = [sql`values.property_id = ${filter.property}`]

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
			const notCondition = buildValueWhere({property: filter.property, text: f.NOT})
			if (notCondition) {
				conditions.push(not(notCondition))
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
			const notCondition = buildValueWhere({property: filter.property, number: f.NOT})
			if (notCondition) {
				conditions.push(not(notCondition))
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

function buildRelationConditions(filter: RelationFilter) {
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

	return conditions.length > 0 ? sql.join(conditions, sql` AND `) : undefined
}

export function buildEntityWhere(filter: EntityFilter | null): SQL | undefined {
	if (!filter) return undefined

	const clauses = []

	if (filter.AND) {
		clauses.push(and(...filter.AND.map(buildEntityWhere)))
	}
	if (filter.OR) {
		clauses.push(or(...filter.OR.map(buildEntityWhere)))
	}
	if (filter.NOT) {
		const notCondition = buildEntityWhere(filter.NOT)
		if (notCondition) {
			clauses.push(not(notCondition))
		}
	}
	if (filter.value) {
		// This checks: exists a value with this filter for the entity
		clauses.push(
			sql`EXISTS (
        SELECT 1 FROM ${values}
        WHERE values.entity_id = entities.id
        AND ${buildValueWhere(filter.value)}
      )`,
		)
	}
	if (filter.fromRelation) {
		// This checks: exists a relation where this entity is the fromEntity
		const relationConditions = buildRelationConditions(filter.fromRelation)
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
	if (filter.toRelation) {
		// This checks: exists a relation where this entity is the toEntity
		const relationConditions = buildRelationConditions(filter.toRelation)
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
