import {readFileSync} from "node:fs"
import {describe, expect, it} from "vitest"
import {type Failure, classify} from "../geodb.js"
import {
	CLIENT_ERRORS,
	CONN_ERRORS,
	GEODB_COMMIT,
	HOST_ERRORS,
	REFUSAL_CATEGORIES,
	REFUSAL_CODES,
} from "./geodbClientErrors.js"

/**
 * The class `classify` must give every failure geodb-client can print. The enumeration is vendored in
 * geodbClientErrors.ts; this table names a class for each name in it, and the coverage tests fail on a name the
 * table lacks, so a new variant (or a pin bump that brings one) cannot reach production unclassified.
 *
 * transient: nothing is lost by repeating the apply (the intent carries an idempotency key, so a repeat of one that
 * landed answers its receipt). exhausted: serve is out of a per-process resource. permanent: a verdict on the edit.
 */
const T: Failure = "transient"
const E: Failure = "exhausted"
const P: Failure = "permanent"

// Printed as `--binding <file>: connect: <name>` (host) or `connect: <name>` (handshake).
const CONNECT: Record<string, Failure> = {
	// the host's failed-connect name: serve restarting, address gone or not listening (#1559)
	IoError: T,
	StoreLocked: T,
	// `connect: Refused` is the node refusing the handshake: nothing listening at the address (#1566)
	Refused: T,
	TransportFailed: T,
	TransportClosed: T,
	DeadlineExceeded: T,
	ProtocolViolation: P,
	WrongState: P,
	InvalidArgument: P,
	ResourceExhausted: E,
}

// Printed as `error: <name>` once a connection stands: the send, the awaited reply, the receive.
const APPLY: Record<string, Failure> = {
	TransportFailed: T,
	TransportClosed: T,
	DeadlineExceeded: T,
	// bare Refused (no refusal frame) is a verdict, unlike `connect: Refused`
	Refused: P,
	ProtocolViolation: P,
	WrongState: P,
	InvalidArgument: P,
	ResourceExhausted: E,
	// an IoError that is not a failed connect is not known to be safe to repeat
	IoError: P,
}

// Raised by the tool around the session: a missing or unusable binding or key is configuration, not an outage.
const CLIENT: Record<string, Failure> = {NoGrant: P, NoListener: P, MissingBinding: P, MissingKey: P}

const CATEGORY: Record<(typeof REFUSAL_CATEGORIES)[number], Failure> = {
	DefiniteRefusal: P,
	NotFoundOrRestricted: P,
	PublishConflict: P,
	OutsideCoverage: P,
	ResourceExhausted: E,
	// geo-db: the caller resolves it by re-sending the same transaction, which is what a repeat is
	OutcomeUnknown: T,
	Corruption: P,
}

// facade/errors.zig `retryable`: PredecessorVersionMismatch, ResourceExhausted, StoreLocked, WatermarkNotYetReached,
// IoError; plus OutcomeUnknown (re-send the same transaction) and DeadlineExceeded (a request whose deadline
// passed, nothing applied).
const CODE_TRANSIENT = [
	"PredecessorVersionMismatch",
	"StoreLocked",
	"WatermarkNotYetReached",
	"IoError",
	"OutcomeUnknown",
	"DeadlineExceeded",
]
const CODE_EXHAUSTED = ["ResourceExhausted"]
const CODE_DEFINITE = [
	"NotFoundOrRestricted",
	"VersionNotFound",
	"StatementNotFound",
	"SpaceNotFound",
	"OutsideCoverage",
	"SnapshotOutsideCoverage",
	"Corruption",
	"EquivocationDetected",
	"CellPreconditionFailed",
	"StatementPreconditionFailed",
	"InvalidArgument",
	"BindingUnassigned",
	"InvalidConfig",
	"IncompatibleFormatVersion",
	"StoreFenced",
	"InvalidCapabilityToken",
	"CapabilityTokenExpired",
	"InvalidHandshakeSignature",
	"TooManySources",
	"TopicTooDeep",
	"DefiniteRefusal",
	"BlobSliceOutOfBounds",
	"InvalidVersionRange",
	"MaskTransitionUnsupported",
	"PropertyIndexPending",
	"PropertyNotIndexed",
	"TopKLimitExceeded",
	"DiffRequiresSingleSpace",
	"AggregateDomainMismatch",
	"AggregateOverflow",
	"ArithmeticOverflow",
	"TypeMismatch",
	"InvalidCast",
	"PlanNodeLimitExceeded",
	"InvalidPlanEncoding",
	"TooManyJoinVariables",
	"TraversalDepthExceeded",
	"InvalidFusionWindow",
	"ExternalScoresMismatch",
	"PhraseNeedsPositions",
	"OntologyClosureTooLarge",
	"FormulaDepthOrCycleExceeded",
	"InvalidCentralityParams",
	"ListTooLong",
	"SubqueryNotScalar",
	"InvalidSortKey",
	"InvalidPointBoxBounds",
	"InvalidCursorKey",
	"CursorSourceVectorMismatch",
	"InvalidDbRef",
	"ConflictingStatementBody",
	"CrossSpaceStatement",
	"ReactivationBelowFloorRefused",
	"StmtOrdExhausted",
	"DraftWithoutBase",
	"CannotDiscardSealedVersion",
	"UnsealedDraftsPresent",
	"NoCommonAncestor",
	"MaxLcaStepsExceeded",
	"InvalidPublishEdit",
	"VersionSeqExhausted",
	"InvalidProvenanceSyntax",
	"UnauthorizedSigner",
	"FutureVersionTimestampRefused",
	"IdempotencyConflict",
	"EditDiscardedByDraftRollback",
	"SpaceOrderExhausted",
	"PinHeldWhileWaiting",
	"InvalidViewPlan",
	"ViewCycleDetected",
	"MaxStrataExceeded",
	"MultiplicityOverflow",
	"NumericAccumulatorOverflow",
	"MalformedRequest",
	"UnitMismatch",
	"InvalidMagic",
	"InvalidHeaderCrc",
	"UnsupportedRevision",
	"UnknownOpcode",
	"UnassignedFlags",
	"FrameTooLarge",
	"TruncatedFrame",
	"TrailingBytes",
	"SequenceMismatch",
	"TagMismatch",
	"InvalidWirePayload",
	"OpcodeNotPermitted",
	"NotAuthenticated",
	"UnknownStream",
	"StreamTableFull",
	"NoFreeRoom",
	"TextKeysPastBound",
	"ProgramUnknown",
	"ProgramRefused",
	"ProgramOverCap",
	"ProgramInputOverCap",
	"GrammarRevisionMismatch",
	"ProofRequired",
	"ChannelNotConfidential",
	"NodeSignatureInvalid",
	"NodeNotPinned",
	"Cancelled",
	"PublishPastEditBound",
]
const CODE: Record<string, Failure> = {
	...Object.fromEntries(CODE_TRANSIENT.map((c) => [c, T])),
	...Object.fromEntries(CODE_EXHAUSTED.map((c) => [c, E])),
	...Object.fromEntries(CODE_DEFINITE.map((c) => [c, P])),
}

// Names from Zig's std and the OS that this client's pin never prints (the host folds them into IoError), kept
// classified in case another build of the client surfaces them.
const STD: Record<string, Failure> = {
	ConnectionRefused: T,
	ConnectionResetByPeer: T,
	BrokenPipe: T,
	EndOfStream: T,
	Timeout: T,
	NetworkUnreachable: T,
	AddressNotAvailable: T,
	UnknownHostName: T,
	TemporaryNameServerFailure: T,
}

const codeCategory = (code: string) =>
	code === "ResourceExhausted" ? "ResourceExhausted" : code === "OutcomeUnknown" ? "OutcomeUnknown" : "DefiniteRefusal"

describe("geodb-client error table: coverage", () => {
	it("names every error the vendored client enumeration lists, and nothing else", () => {
		expect(Object.keys(CODE).sort()).toEqual([...REFUSAL_CODES].sort())
		expect(Object.keys(CATEGORY).sort()).toEqual([...REFUSAL_CATEGORIES].sort())
		expect(Object.keys(CLIENT).sort()).toEqual([...CLIENT_ERRORS].sort())
		// APPLY and CONNECT name the connection's errors; the host's IoError also appears bare after a connect.
		expect(Object.keys(APPLY).sort()).toEqual([...new Set([...CONN_ERRORS, "IoError"])].sort())
		expect(Object.keys(CONNECT).sort()).toEqual([...new Set([...CONN_ERRORS, ...HOST_ERRORS])].sort())
	})

	it("is derived from the geo-db commit the cluster pins", () => {
		const yaml = readFileSync(new URL("../../../geodb/k8s/v2/geodb.yaml", import.meta.url), "utf8")
		const tags = [...yaml.matchAll(/image: \S+\/geodb:([0-9a-f]+)/g)].map((m) => m[1])
		expect(tags.length).toBeGreaterThan(0)
		// A bump means re-reading the client's error sets into geodbClientErrors.ts before this passes.
		for (const tag of tags) expect(tag).toBe(GEODB_COMMIT)
	})
})

describe("geodb-client error table: classify", () => {
	for (const [name, want] of Object.entries(CONNECT)) {
		it(`connect: ${name} is ${want}`, () => {
			const text = (HOST_ERRORS as readonly string[]).includes(name)
				? `--binding /geodb/binding.txt: connect: ${name}\n`
				: `connect: ${name}\n`
			expect(classify(text)).toBe(want)
		})
	}
	for (const [name, want] of Object.entries(APPLY)) {
		it(`apply error: ${name} is ${want}`, () => expect(classify(`error: ${name}\n`)).toBe(want))
	}
	for (const [name, want] of Object.entries(CLIENT)) {
		it(`client error: ${name} is ${want}`, () => expect(classify(`error: ${name}\n`)).toBe(want))
	}
	for (const [category, want] of Object.entries(CATEGORY)) {
		it(`refused category ${category} is ${want}`, () => expect(classify(`refused: ${category} -\n`)).toBe(want))
	}
	for (const [code, want] of Object.entries(CODE)) {
		it(`refused code ${code} is ${want}`, () =>
			expect(classify(`refused: ${codeCategory(code)} ${code}\n`)).toBe(want))
	}
	for (const [name, want] of Object.entries(STD)) {
		it(`std name ${name} is ${want}`, () => expect(classify(`error: ${name}\n`)).toBe(want))
	}
})
