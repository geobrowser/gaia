// Fails when an api `__tests__` folder (at any depth) is not named by a path in an
// API test workflow. The unit workflow excludes **/__tests__/**, so a folder that no
// workflow lists runs nowhere. vitest filters are substring matches, so a listed
// `src/geodb/__tests__/` does NOT cover `src/geodb/rankedReads/__tests__/`; a folder
// counts as covered only when a listed path is a prefix of it.
import {readdirSync, readFileSync} from "node:fs"
import {join} from "node:path"

const WORKFLOWS = ["../.github/workflows/api-integration-tests.yml"]

function findTestDirs(dir: string, out: string[] = []): string[] {
	for (const entry of readdirSync(dir, {withFileTypes: true})) {
		if (!entry.isDirectory() || entry.name === "node_modules") continue
		const path = join(dir, entry.name)
		if (entry.name === "__tests__") out.push(path)
		else findTestDirs(path, out)
	}
	return out
}

const listed = WORKFLOWS.flatMap((file) =>
	readFileSync(file, "utf8")
		.split("\n")
		.filter((line) => !line.trimStart().startsWith("#"))
		.flatMap((line) => line.match(/\bsrc\/[^\s"']*__tests__\/[^\s"']*/g) ?? []),
)

const missing = findTestDirs("src")
	.sort()
	.filter((dir) => !listed.some((path) => `${dir}/`.startsWith(path.endsWith("/") ? path : `${path}/`)))

if (missing.length > 0) {
	console.error("These __tests__ folders are listed in no API test workflow, so they run nowhere:")
	for (const dir of missing) console.error(`  ${dir}`)
	console.error("Add each to the folder list in .github/workflows/api-integration-tests.yml (or the pg-free-tests job).")
	process.exit(1)
}
console.log(`ok: all ${findTestDirs("src").length} __tests__ folders are listed`)
