/**
 * Postgres read of one entity's card: its values and its outgoing relations in one space.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"

import type {RelationRow, ValueRow} from "./card"

type Database = NodePgDatabase<Record<string, unknown>>

export async function getCardRows(
	db: Database,
	entityId: string,
	spaceId: string,
): Promise<{values: ValueRow[]; relations: RelationRow[]}> {
	const [values, relations] = await Promise.all([
		db.execute<ValueRow>(sql`
			SELECT property_id::text AS property_id, text, integer::text AS integer, float::text AS float,
			       decimal::text AS decimal, boolean, datetime, date
			FROM "values"
			WHERE entity_id = ${entityId}::uuid AND space_id = ${spaceId}::uuid
		`),
		db.execute<RelationRow>(sql`
			SELECT type_id::text AS type_id, to_entity_id::text AS to_entity_id, to_space_id::text AS to_space_id
			FROM relations
			WHERE from_entity_id = ${entityId}::uuid AND space_id = ${spaceId}::uuid
		`),
	])
	return {values: values.rows, relations: relations.rows}
}
