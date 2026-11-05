/**
 * The membership, votes and proposal-votes backfill Jobs take their geodb (geodb-client) image from geodb.yaml at apply time, through
 * the GEODB_TAG placeholder, so they cannot drift behind the serve they write to (a client older than geo-db
 * 9d97069a78 refuses every keyed `put`). This holds the manifests to that contract.
 */
import {readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {describe, expect, it} from "vitest"

const k8s = join(dirname(fileURLToPath(import.meta.url)), "../../../geodb/k8s/v2")
const read = (f: string) => readFileSync(join(k8s, f), "utf8")
const geodbImages = (yaml: string) => [...yaml.matchAll(/^ *image: registry\.digitalocean\.com\/geo\/geodb:(\S+)$/gm)].map((m) => m[1])

describe("backfill Job geodb image", () => {
	it("geodb.yaml pins exactly one geodb tag, so the Jobs' derivation is unambiguous", () => {
		const tags = new Set(geodbImages(read("geodb.yaml")))
		expect(tags.size).toBe(1)
		expect([...tags][0]).toMatch(/^[0-9a-f]{10}$/)
	})

	it.each(["geodb-votes-backfill-job.yaml", "geodb-proposal-votes-backfill-job.yaml", "geodb-membership-backfill-job.yaml"])(
		"%s uses the GEODB_TAG placeholder, fills it from geodb.yaml, and warns about put",
		(f) => {
			const job = read(f)
			expect(geodbImages(job)).toEqual(["GEODB_TAG"])
			expect(job).toContain("geodb.yaml | sort -u)")
			expect(job).toContain('-e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|"')
			expect(job).toContain("9d97069a78")
			// What the header's sed does to the image line.
			const filled = job.replaceAll("geodb:GEODB_TAG", `geodb:${geodbImages(read("geodb.yaml"))[0]}`)
			expect(geodbImages(filled)).toEqual(geodbImages(read("geodb.yaml")).slice(0, 1))
		},
	)
})
