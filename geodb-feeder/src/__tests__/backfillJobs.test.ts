/**
 * The membership, votes and proposal-votes backfill Jobs take their geodb (geodb-client) image from geodb.yaml at apply time, through
 * the GEODB_TAG placeholder, so they cannot drift behind the serve they write to (a client older than geo-db
 * 9d97069a78 refuses every keyed `put`). This holds the manifests to that contract.
 */
import {execFileSync} from "node:child_process"
import {readdirSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {describe, expect, it} from "vitest"

const k8s = join(dirname(fileURLToPath(import.meta.url)), "../../../geodb/k8s/v2")
const read = (f: string) => readFileSync(join(k8s, f), "utf8")
const geodbImages = (yaml: string) => [...yaml.matchAll(/^ *image: registry\.digitalocean\.com\/geo\/geodb:(\S+)$/gm)].map((m) => m[1])

// The serve container's tag: the client must match serve. geodb.yaml also pins an older tag on the feeder's geodb-setup
// init container (left behind by targeted serve bumps), so the Jobs must not take "the" tag from every image line.
const serveTag = () => {
	const yaml = read("geodb.yaml")
	const at = yaml.search(/^ *- name: serve$/m)
	return /image: registry\.digitalocean\.com\/geo\/geodb:(\S+)/.exec(yaml.slice(at))?.[1]
}

describe("backfill Job geodb image", () => {
	it("geodb.yaml's serve container is pinned to a 10-hex geodb tag", () => {
		expect(serveTag()).toMatch(/^[0-9a-f]{10}$/)
	})

	it.each(readdirSync(k8s).filter((f) => f.endsWith(".yaml") && read(f).includes("geodb:GEODB_TAG")))(
		"%s derives the serve container's tag, fills the placeholder, and warns about put",
		(f) => {
			const job = read(f)
			expect(geodbImages(job)).toEqual(["GEODB_TAG"])
			expect(job).toContain('-e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|"')
			if (/votes|membership|ranks/.test(f)) expect(job).toContain("9d97069a78")
			// Run the header's derivation against geodb.yaml: it must yield exactly the serve tag.
			const derive = /GEODB_TAG=\$\((awk '.*' geodb\.yaml)\)/.exec(job)?.[1]
			expect(derive, "derivation line").toBeTruthy()
			const got = execFileSync("sh", ["-c", derive as string], {cwd: k8s, encoding: "utf8"}).trim()
			expect(got).toBe(serveTag())
		},
	)
})
