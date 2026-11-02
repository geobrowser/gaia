/**
 * geo-db read client for entity cards.
 *
 * Stage 1 reaches geo-db the way the shadow harness does: through the `geodb-client` binary, one
 * `query` per card over the binding's listener. It is a seam (`GeoDbCardClient`) so a native wire
 * client can replace it without touching the route or the fallback logic.
 */

import {execFile} from "node:child_process"
import {readFile} from "node:fs/promises"
import {promisify} from "node:util"
import {rowsFromGeoDbOutput} from "./card"
import type {GeoDbCardConfig} from "./config"

const execFileAsync = promisify(execFile)

const HEX32 = /^[0-9a-f]{32}$/

/** Rows of one entity's card as geo-db answered them, in geo-db's id space. */
export type GeoDbCardClient = {
	readCard(args: {geoSpaceId: string; entityId: string}): Promise<{prop: string; raw: string}[]>
}

export class GeoDbError extends Error {
	constructor(
		readonly reason: "timeout" | "unmapped_space" | "client_failed" | "bad_request",
		message: string,
	) {
		super(message)
		this.name = "GeoDbError"
	}
}

/** The geoql document for one entity's card. Ids are 32 hex digits, so nothing needs escaping. */
export function cardDocument(entityId: string): string {
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entityId}" } } } }`
}

/** gaia space id (compact) -> geo-db space id, from the shadow loader's state file. */
export function createSpaceMap(path: string): (gaiaSpaceId: string) => Promise<string | null> {
	let loaded: Promise<Record<string, {geo?: string}>> | null = null
	return async (gaiaSpaceId) => {
		if (!path) return null
		loaded ??= readFile(path, "utf8").then((text) => JSON.parse(text) as Record<string, {geo?: string}>)
		try {
			return (await loaded)[gaiaSpaceId]?.geo ?? null
		} catch (error) {
			loaded = null // retry the read on the next request
			throw new GeoDbError("client_failed", `space map unreadable: ${String(error)}`)
		}
	}
}

export function createCliGeoDbClient(config: GeoDbCardConfig): GeoDbCardClient {
	return {
		async readCard({geoSpaceId, entityId}) {
			if (!HEX32.test(geoSpaceId) || !HEX32.test(entityId)) {
				throw new GeoDbError("bad_request", "ids must be 32 hex digits")
			}
			try {
				const {stdout} = await execFileAsync(
					config.clientBin,
					[
						"--key",
						config.keyPath,
						"--binding",
						config.bindingPath,
						"query",
						"--space",
						geoSpaceId,
						cardDocument(entityId),
					],
					{timeout: config.timeoutMs, killSignal: "SIGKILL", maxBuffer: 16 * 1024 * 1024, encoding: "utf8"},
				)
				return rowsFromGeoDbOutput(stdout, entityId)
			} catch (error) {
				const failure = error as {killed?: boolean; signal?: string; stderr?: string}
				if (failure.killed || failure.signal === "SIGKILL") {
					throw new GeoDbError("timeout", `geodb-client exceeded ${config.timeoutMs} ms`)
				}
				throw new GeoDbError("client_failed", String(failure.stderr || error).slice(0, 300))
			}
		},
	}
}
