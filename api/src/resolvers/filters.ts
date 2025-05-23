import {and, eq, isNull, like, not, or, sql} from "drizzle-orm"
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
	propertyId: string
	text?: TextFilter
	number?: NumberFilter
	checkbox?: CheckboxFilter
	point?: PointFilter
}

export type EntityFilter = {
	AND?: EntityFilter[]
	OR?: EntityFilter[]
	NOT?: EntityFilter
	value?: PropertyFilter
}

function buildValueWhere(filter: PropertyFilter): any {
	const conditions: any[] = [eq(values.propertyId, filter.propertyId)]

	if (filter.text) {
		const f = filter.text
		if (f.is !== undefined) conditions.push(eq(values.value, f.is))
		if (f.contains !== undefined) conditions.push(like(values.value, `%${f.contains}%`))
		if (f.startsWith !== undefined) conditions.push(like(values.value, `${f.startsWith}%`))
		if (f.endsWith !== undefined) conditions.push(like(values.value, `%${f.endsWith}`))
		if (f.exists !== undefined) {
			conditions.push(f.exists ? not(isNull(values.value)) : isNull(values.value))
		}
		if (f.NOT) {
			conditions.push(not(buildValueWhere({propertyId: filter.propertyId, text: f.NOT})))
		}
	}

	if (filter.number) {
		const f = filter.number
		// Use CASE to safely cast, returning NULL for non-numeric values
		const safeCasted = sql`CASE 
        WHEN ${values.value} ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'
 
        THEN ${values.value}::numeric 
        ELSE NULL 
    END`

		if (f.is !== undefined) conditions.push(sql`${safeCasted} = ${f.is}`)
		if (f.lessThan !== undefined) conditions.push(sql`${safeCasted} < ${f.lessThan}`)
		if (f.lessThanOrEqual !== undefined) conditions.push(sql`${safeCasted} <= ${f.lessThanOrEqual}`)
		if (f.greaterThan !== undefined) conditions.push(sql`${safeCasted} > ${f.greaterThan}`)
		if (f.greaterThanOrEqual !== undefined) conditions.push(sql`${safeCasted} >= ${f.greaterThanOrEqual}`)

		if (f.exists !== undefined) {
			// For exists, check if the value exists AND is numeric
			const isNumeric = sql`${values.value} ~ '^-?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$'`

			conditions.push(
				f.exists ? and(not(isNull(values.value)), isNumeric) : or(isNull(values.value), not(isNumeric)),
			)
		}

		if (f.NOT) {
			conditions.push(not(buildValueWhere({propertyId: filter.propertyId, number: f.NOT})))
		}
	}

	if (filter.checkbox) {
		const f = filter.checkbox
		if (f.is !== undefined) conditions.push(eq(values.value, f.is.toString()))
		if (f.exists !== undefined) {
			conditions.push(f.exists ? not(isNull(values.value)) : isNull(values.value))
		}
	}

	if (filter.point) {
		const f = filter.point
		if (f.is !== undefined) conditions.push(eq(values.value, JSON.stringify(f.is)))
		if (f.exists !== undefined) {
			conditions.push(f.exists ? not(isNull(values.value)) : isNull(values.value))
		}
	}

	return and(...conditions)
}

export function buildEntityWhere(filter: EntityFilter | null): any | undefined {
	if (!filter) return undefined

	const clauses: any[] = []

	if (filter.AND) {
		clauses.push(and(...filter.AND.map(buildEntityWhere)))
	}
	if (filter.OR) {
		clauses.push(or(...filter.OR.map(buildEntityWhere)))
	}
	if (filter.NOT) {
		clauses.push(not(buildEntityWhere(filter.NOT)))
	}
	if (filter.value) {
		// This checks: exists a value with this filter for the entity
		clauses.push(
			sql`EXISTS (
        SELECT 1 FROM ${values} 
        WHERE ${values.entityId} = ${entities.id}
        AND ${buildValueWhere(filter.value)}
      )`,
		)
	}

	return and(...clauses)
}
