import {chmodSync, mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cardDocument, createCliGeoDbClient, createSpaceMap, GeoDbError} from "../client"
import type {GeoDbCardConfig} from "../config"

const ENTITY = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "22222222222242228222222222222222"
const PROP = "98012398829d48b083738f2589675894"

function fakeClient(script: string, timeoutMs = 2000) {
	const dir = mkdtempSync(join(tmpdir(), "geodb-client-"))
	const bin = join(dir, "geodb-client")
	writeFileSync(bin, `#!/bin/sh\n${script}\n`)
	chmodSync(bin, 0o755)
	const config: GeoDbCardConfig = {
		mode: "on",
		clientBin: bin,
		keyPath: "k",
		bindingPath: "b",
		spaceMapPath: "",
		timeoutMs,
	}
	return createCliGeoDbClient(config)
}

describe("createCliGeoDbClient", () => {
	it("passes the query on the command line and parses the rows", async () => {
		const client = fakeClient(`printf '%s\\n' "$*" >&2; printf '${PROP}\\tNoam\\tstmt\\t${ENTITY}\\t1\\n'`)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "Noam"}])
	})

	it("builds the entity card document", () => {
		expect(cardDocument(ENTITY)).toContain(`.id = "${ENTITY}"`)
	})

	it("times out", async () => {
		const client = fakeClient("sleep 5", 100)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({reason: "timeout"})
	})

	it("reports a failing client", async () => {
		const client = fakeClient("echo 'refused: auth' >&2; exit 1")
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
		})
	})

	it("refuses ids that are not 32 hex digits before running anything", async () => {
		const client = fakeClient("exit 0")
		await expect(client.readCard({geoSpaceId: SPACE, entityId: 'x" } }'})).rejects.toBeInstanceOf(GeoDbError)
	})
})

describe("createSpaceMap", () => {
	it("is empty without a path", async () => {
		expect(await createSpaceMap("")(SPACE)).toBeNull()
	})

	it("reads the shadow loader's state file", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY, nonce: "n"}}))
		const lookup = createSpaceMap(path)
		expect(await lookup(SPACE)).toBe(ENTITY)
		expect(await lookup(ENTITY)).toBeNull()
	})

	it("fails loudly when the file is unreadable", async () => {
		await expect(createSpaceMap("/nonexistent/spaces.json")(SPACE)).rejects.toMatchObject({reason: "client_failed"})
	})
})
