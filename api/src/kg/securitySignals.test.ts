import {buildSchema, parse} from "graphql"
import {beforeEach, describe, expect, it} from "vitest"
import {
	__resetSecuritySignalMetricsForTests,
	ALL_HIDDEN_TARGETS,
	analyzeRejectedDocument,
	countHiddenSurfaceProbe,
	countInternalError,
	countIntrospection,
	countRejection,
	countSuppressedLogLine,
	detectIntrospection,
	HIDDEN_SURFACE,
	renderSecuritySignalMetrics,
	selectedOperationTargetsMissingRoot,
	targetsMissingRootType,
} from "./securitySignals"

// Shaped like the live schema where it matters: a Query root, Node interface, and — since gaia#1010
// — no Mutation type at all.
const schema = buildSchema(`
	interface Node { nodeId: ID! }
	type Space implements Node { nodeId: ID! id: ID! name: String }
	type Query {
		query: Query!
		node(nodeId: ID!): Node
		spaces(first: Int): [Space!]
		space(id: ID!): Space
	}
`)

const analyze = (source: string) => analyzeRejectedDocument(schema, parse(source))

describe("analyzeRejectedDocument", () => {
	it("names unknown fields with their parent type", () => {
		const a = analyze(`{ spaces { id secretSauce } usersConnection { totalCount } }`)
		// Breadth-first: root-level names before nested ones.
		expect(a.unknownFields).toEqual(["Query.usersConnection", "Space.secretSauce"])
		expect(a.hiddenSurfaceHits).toEqual([])
	})

	it("flags every inflection of a hidden table as one canonical target", () => {
		const a = analyze(`{
			appWebhooksConnection { totalCount }
			appWebhookByAppName(appName: "x") { secret }
			notificationOutboxes { payload }
			notificationDeliveriesConnection { totalCount }
			notificationPollCursorByNodeId(nodeId: "x") { id }
		}`)
		expect(a.hiddenSurfaceHits).toEqual([
			"Query.appWebhook",
			"Query.notificationOutbox",
			"Query.notificationDeliver",
			"Query.notificationPollCursor",
		])
	})

	it("flags hidden mutations even though the schema has no Mutation type", () => {
		const a = analyze(`mutation { refreshSpaceTopicSuggestions(input: {}) { clientMutationId } }`)
		expect(a.unknownFields).toEqual(["Mutation.refreshSpaceTopicSuggestions"])
		expect(a.hiddenSurfaceHits).toEqual(["Mutation.refreshSpaceTopicSuggestions"])
	})

	it("reports unknown mutations without calling them hidden", () => {
		const a = analyze(`mutation { deleteEverything { ok } }`)
		expect(a.unknownFields).toEqual(["Mutation.deleteEverything"])
		expect(a.hiddenSurfaceHits).toEqual([])
	})

	it("flags a fragment on a hidden type, the shape of a node(nodeId) probe", () => {
		const a = analyze(`{ node(nodeId: "x") { ... on AppWebhook { secret url } } }`)
		expect(a.unknownTypes).toEqual(["AppWebhook"])
		expect(a.hiddenSurfaceHits).toEqual(["type:AppWebhook"])
	})

	it("flags named fragments on hidden types", () => {
		const a = analyze(`query { node(nodeId: "x") { ...W } } fragment W on NotificationOutbox { payload }`)
		expect(a.hiddenSurfaceHits).toEqual(["type:NotificationOutbox"])
	})

	it("still matches hidden root fields inside a fragment on the root type", () => {
		const a = analyze(`{ ... on Query { appWebhooks { secret } } }`)
		expect(a.hiddenSurfaceHits).toEqual(["Query.appWebhook"])
		const b = analyze(`query { ...Root } fragment Root on Query { notificationOutboxes { payload } }`)
		expect(b.hiddenSurfaceHits).toEqual(["Query.notificationOutbox"])
	})

	it("keeps mutation-root context in fragments on a Mutation type the schema lacks", () => {
		const inline = analyze(`mutation { ... on Mutation { sampleFeedComposition(input: {}) { clientMutationId } } }`)
		expect(inline.hiddenSurfaceHits).toEqual(["Mutation.sampleFeedComposition"])
		const named = analyze(
			`mutation { ...M } fragment M on Mutation { refreshSpaceTopicSuggestions { clientMutationId } }`,
		)
		expect(named.hiddenSurfaceHits).toEqual(["Mutation.refreshSpaceTopicSuggestions"])
		const unused = analyze(
			`{ spaces { id } } fragment M on Mutation { recordFeedCompositionSample { clientMutationId } }`,
		)
		expect(unused.hiddenSurfaceHits).toEqual(["Mutation.recordFeedCompositionSample"])
	})

	it("keeps every hidden target, even when a document names all of them", () => {
		// Built from HIDDEN_SURFACE rather than written out, so it keeps naming every target as the
		// list grows: one root field per prefix, one inline fragment per type, one mutation each.
		const doc = `
			query { ${HIDDEN_SURFACE.queryFieldPrefixes.map((p) => `${p}s { id }`).join(" ")}
				node(nodeId: "x") { ${HIDDEN_SURFACE.types.map((t) => `... on ${t} { id }`).join(" ")} } }
			fragment M on Mutation { ${HIDDEN_SURFACE.mutations.join(" ")} }`
		const a = analyze(doc)
		expect(a.hiddenSurfaceHits).toHaveLength(ALL_HIDDEN_TARGETS.length)
		expect([...a.hiddenSurfaceHits].sort()).toEqual([...ALL_HIDDEN_TARGETS].sort())
	})

	it("treats an invented __ field as unknown, and only real meta-fields as exempt", () => {
		expect(analyze(`{ __notAField }`).unknownFields).toEqual(["Query.__notAField"])
		expect(analyze(`{ spaces { __schema { types { name } } } }`).unknownFields).toEqual(["Space.__schema"])
		expect(analyze(`{ __schema { types { name } } __typename spaces { __typename bogus } }`).unknownFields).toEqual(
			["Space.bogus"],
		)
	})

	it("is not fooled by a duplicate fragment name shadowing a probe", () => {
		const a = analyze(
			`query { ...F } fragment F on Query { appWebhooks { id } } fragment F on Query { __typename }`,
		)
		expect(a.hiddenSurfaceHits).toEqual(["Query.appWebhook"])
		const b = analyze(
			`query { ...F } fragment F on Query { __typename } fragment F on Query { appWebhooks { id } }`,
		)
		expect(b.hiddenSurfaceHits).toEqual(["Query.appWebhook"])
	})

	it("has no depth cutoff to hide behind", () => {
		const depth = 500
		const doc = `{ ${"... on Query { ".repeat(depth)} appWebhooks { id } ${"} ".repeat(depth)}}`
		expect(analyze(doc).hiddenSurfaceHits).toEqual(["Query.appWebhook"])
		const fieldDepth = `{ node(nodeId: "x") { ${"... on Space { ".repeat(depth)} ... on AppWebhook { secret } ${"} ".repeat(depth)}} }`
		expect(analyze(fieldDepth).hiddenSurfaceHits).toEqual(["type:AppWebhook"])
	})

	it("walks inside __schema and __type instead of skipping them", () => {
		expect(analyze(`{ __schema { bogus } }`).unknownFields).toEqual(["__Schema.bogus"])
		expect(analyze(`{ __schema { ... on AppWebhook { secret } } }`).hiddenSurfaceHits).toEqual(["type:AppWebhook"])
		expect(analyze(`{ __type(name: "Space") { nope } }`).unknownFields).toEqual(["__Type.nope"])
	})

	it("checks variable declarations for hidden types, wrapped or not", () => {
		expect(analyze(`query Probe($x: AppWebhook) { __typename }`).hiddenSurfaceHits).toEqual(["type:AppWebhook"])
		expect(analyze(`query Probe($x: [NotificationOutbox!]!) { __typename }`).hiddenSurfaceHits).toEqual([
			"type:NotificationOutbox",
		])
		expect(analyze(`query Ok($n: Int, $id: ID!) { space(id: $id) { id } }`).unknownTypes).toEqual([])
	})

	it("treats selections on a nested Query (PostGraphile's query: Query!) as root selections", () => {
		expect(analyze(`{ query { appWebhooks { id } } }`).hiddenSurfaceHits).toEqual(["Query.appWebhook"])
		expect(analyze(`{ query { query { notificationOutboxes { id } } } }`).hiddenSurfaceHits).toEqual([
			"Query.notificationOutbox",
		])
		// And the meta-fields stay exempt there, as graphql-js resolves them on Query at any depth.
		expect(analyze(`{ query { __schema { bogus } } }`).unknownFields).toEqual(["__Schema.bogus"])
	})

	it("walks an invalid selection set under __typename for hidden type conditions", () => {
		expect(analyze(`{ __typename { ... on AppWebhook { secret } } }`).hiddenSurfaceHits).toEqual([
			"type:AppWebhook",
		])
	})

	it("walks subscriptions, reporting fields on an absent root as unknown", () => {
		expect(analyze(`subscription { doesNotExist }`).unknownFields).toEqual(["Subscription.doesNotExist"])
		const probe = analyze(`subscription { node(nodeId: "x") { ... on AppWebhook { secret } } }`)
		expect(probe.hiddenSurfaceHits).toEqual(["type:AppWebhook"])
	})

	it("survives a fragment cycle without looping", () => {
		const a = analyze(`query { ...A } fragment A on Query { ...B } fragment B on Query { ...A appWebhooks { id } }`)
		expect(a.hiddenSurfaceHits).toEqual(["Query.appWebhook"])
	})

	it("does not count a field named like a hidden table when it is nested, not a root field", () => {
		const a = analyze(`{ spaces { appWebhooks { id } } }`)
		expect(a.unknownFields).toEqual(["Space.appWebhooks"])
		expect(a.hiddenSurfaceHits).toEqual([])
	})

	it("caps what it reports, so a hostile document cannot bloat a log line", () => {
		const fields = Array.from({length: 500}, (_, i) => `f${i}`).join(" ")
		const a = analyze(`{ ${fields} }`)
		expect(a.unknownFields).toHaveLength(10)
	})

	it("truncates absurdly long names", () => {
		const a = analyze(`{ ${"x".repeat(5000)} }`)
		expect(a.unknownFields[0]?.length ?? 0).toBeLessThan(80)
	})

	it("finds nothing in a document that only fails for other reasons", () => {
		const a = analyze(`{ spaces(first: "not an int") { id } }`)
		expect(a).toEqual({unknownFields: [], unknownTypes: [], hiddenSurfaceHits: []})
	})
})

describe("targetsMissingRootType", () => {
	it("is true for any mutation when the schema has no Mutation type", () => {
		expect(targetsMissingRootType(schema, parse(`mutation { anything }`))).toBe(true)
		expect(targetsMissingRootType(schema, parse(`subscription { anything }`))).toBe(true)
	})

	it("is false for queries", () => {
		expect(targetsMissingRootType(schema, parse(`{ spaces { id } }`))).toBe(false)
	})
})

describe("selectedOperationTargetsMissingRoot", () => {
	const doc = parse(`query Read { spaces { id } } mutation Unused { bogus }`)

	it("is false when the operation that runs is a query, even beside an unused mutation", () => {
		expect(selectedOperationTargetsMissingRoot(schema, doc, "Read")).toBe(false)
	})

	it("is true when the operation that runs is the mutation", () => {
		expect(selectedOperationTargetsMissingRoot(schema, doc, "Unused")).toBe(true)
		expect(selectedOperationTargetsMissingRoot(schema, parse(`mutation { x }`))).toBe(true)
	})

	it("is false when execution cannot pick an operation at all", () => {
		expect(selectedOperationTargetsMissingRoot(schema, doc)).toBe(false)
	})
})

describe("detectIntrospection", () => {
	it("detects __schema and __type at the top level", () => {
		expect(detectIntrospection(parse(`{ __schema { types { name } } }`))).toEqual(["schema"])
		expect(detectIntrospection(parse(`{ __type(name: "Space") { name } }`))).toEqual(["type"])
	})

	it("follows a top-level fragment spread, as GraphiQL's introspection query does", () => {
		const doc = parse(
			`query IntrospectionQuery { ...Root } fragment Root on Query { __schema { queryType { name } } }`,
		)
		expect(detectIntrospection(doc)).toEqual(["schema"])
	})

	it("follows root-level fragment spreads transitively", () => {
		const doc = parse(
			`query { ...A } fragment A on Query { ...B } fragment B on Query { __schema { queryType { name } } }`,
		)
		expect(detectIntrospection(doc)).toEqual(["schema"])
		const cycle = parse(
			`query { ...A } fragment A on Query { ...B } fragment B on Query { ...A __type(name: "X") { name } }`,
		)
		expect(detectIntrospection(cycle)).toEqual(["type"])
	})

	it("finds introspection on a nested Query when given the schema", () => {
		expect(detectIntrospection(parse(`{ query { __schema { queryType { name } } } }`), null, schema)).toEqual([
			"schema",
		])
		expect(
			detectIntrospection(
				parse(`{ query { ...Q } } fragment Q on Query { query { __type(name: "Space") { name } } }`),
				null,
				schema,
			),
		).toEqual(["type"])
		// Never descends into fields of other types.
		expect(detectIntrospection(parse(`{ spaces { __typename } }`), null, schema)).toEqual([])
		// Without a schema, only the top level is checked.
		expect(detectIntrospection(parse(`{ query { __schema { queryType { name } } } }`))).toEqual([])
	})

	it("counts only the operation that will execute", () => {
		const doc = parse(`query A { spaces { id } } query B { __schema { queryType { name } } }`)
		expect(detectIntrospection(doc, "A")).toEqual([])
		expect(detectIntrospection(doc, "B")).toEqual(["schema"])
		// No operationName with two operations: execution refuses to pick one, so nothing ran.
		expect(detectIntrospection(doc)).toEqual([])
		expect(detectIntrospection(doc, "Missing")).toEqual([])
	})

	it("ignores __typename and ordinary queries", () => {
		expect(detectIntrospection(parse(`{ spaces { __typename id } }`))).toEqual([])
		expect(detectIntrospection(parse(`{ __typename }`))).toEqual([])
	})
})

describe("renderSecuritySignalMetrics", () => {
	beforeEach(__resetSecuritySignalMetricsForTests)

	it("always exposes every reason and kind, so absent series never break a rate()", () => {
		const out = renderSecuritySignalMetrics({sources: 0, evictions: 0})
		expect(out).toContain('gaia_api_graphql_rejected_total{reason="parse"} 0')
		expect(out).toContain('gaia_api_graphql_rejected_total{reason="unknown_field"} 0')
		expect(out).toContain('gaia_api_graphql_rejected_total{reason="other_validation"} 0')
		expect(out).toContain('gaia_api_graphql_introspection_total{kind="schema"} 0')
		expect(out).toContain("# TYPE gaia_api_graphql_hidden_surface_probe_total counter")
		// Hidden targets exist from the start too — the first probe after a deploy must be an increase.
		expect(out).toContain('gaia_api_graphql_hidden_surface_probe_total{target="Query.appWebhook"} 0')
		expect(out).toContain('gaia_api_graphql_hidden_surface_probe_total{target="Mutation.sampleFeedComposition"} 0')
		expect(out).toContain('gaia_api_graphql_hidden_surface_probe_total{target="type:AppWebhook"} 0')
	})

	it("refuses label values outside the fixed set", () => {
		countHiddenSurfaceProbe('Query.evil"} 1\nfake_metric{a="')
		const out = renderSecuritySignalMetrics({sources: 0, evictions: 0})
		expect(out).not.toContain("evil")
		expect(out).not.toContain("fake_metric")
	})

	it("renders counts in Prometheus text format", () => {
		countRejection("parse")
		countRejection("unknown_field")
		countRejection("unknown_field")
		countHiddenSurfaceProbe("Query.appWebhook")
		countIntrospection("schema")
		countSuppressedLogLine()
		countInternalError()
		const out = renderSecuritySignalMetrics({sources: 7, evictions: 2})
		expect(out).toContain("gaia_api_security_signal_internal_errors_total 1")
		expect(out).toContain('gaia_api_graphql_rejected_total{reason="unknown_field"} 2')
		expect(out).toContain('gaia_api_graphql_hidden_surface_probe_total{target="Query.appWebhook"} 1')
		expect(out).toContain('gaia_api_graphql_introspection_total{kind="schema"} 1')
		expect(out).toContain("gaia_api_security_signal_log_suppressed_total 1")
		expect(out).toContain("gaia_api_security_signal_sources 7")
		expect(out).toContain("gaia_api_security_signal_source_evictions_total 2")
		// Every sample line is `name{labels} value` or `name value`.
		for (const line of out.trim().split("\n")) {
			if (!line.startsWith("#")) expect(line).toMatch(/^[a-z_]+(\{[a-z]+="[^"]*"\})? \d+$/)
		}
	})
})
