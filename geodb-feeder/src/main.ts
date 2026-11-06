/**
 * geodb-feeder: topic `<env>.knowledge.edits` -> HermesEdit -> GRC-20 ops -> geodb-client apply -> geodb-serve.
 *
 * Delivery is at-least-once with a resumable apply: see state.ts for what is stored and why a replayed
 * message is safe. The feeder is its own Deployment and reaches geodb-serve over TCP; it founds Spaces online.
 */
import {existsSync, readFileSync} from "node:fs"
import * as zstd from "@bokuweb/zstd-wasm"
import * as grc from "@geoprotocol/grc-20"
import kafkajs from "kafkajs"
import {createLock} from "./apply.js"
import {type Config, loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {type Decode, handleMessage} from "./handle.js"
import {errorFields, log} from "./log.js"
import {compareMembership} from "./membershipShadow.js"
import {MembershipState} from "./membership.js"
import {startMembershipConsumer} from "./membershipConsumer.js"
import {keyedCompareTarget, keyedMembershipEnabled, keyedOffsetsDir} from "./membershipKeyed.js"
import {ProposalOffsets} from "./proposals.js"
import {startProposalConsumer} from "./proposalsConsumer.js"
import {startOptionalConsumer} from "./optionalConsumer.js"
import {startProposalActionsConsumer} from "./proposalActionsConsumer.js"
import {proposalActionsKeyedEnabled, proposalActionsOffsetsDir} from "./proposalActionsKeyed.js"
import {startProposalVotesConsumer} from "./proposalVotesConsumer.js"
import {proposalVotesKeyedEnabled, proposalVotesOffsetsDir} from "./proposalVotesKeyed.js"
import {startVotesConsumer} from "./votesConsumer.js"
import {votesKeyedEnabled, votesOffsetsDir} from "./votesKeyed.js"
import {PendingSpaces} from "./spaceEntities.js"
import {startSpaceEntityConsumer} from "./spaceEntitiesConsumer.js"
import {createMetrics, observeFreshness} from "./metrics.js"
import {PipeReader} from "./geodbPipe.js"
import {ReadOnlyPg, pgConfig, readMembershipIds} from "./pgSource.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {SYSTEM_SPACE_MAP_KEY, SystemSpaces} from "./systemSpaces.js"
import {startServer} from "./server.js"
import {ensureSpace, readSealer, SpaceCapError} from "./spaces.js"
import {Store, backfilledSpaces, renderSpaceMap} from "./state.js"
import {createGroupCommitter} from "./groupCommit.js"

const {Kafka, logLevel, CompressionTypes, CompressionCodecs} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

async function main(config: Config): Promise<void> {
	const metrics = createMetrics()
	const abort = new AbortController()
	let consumerRunning = false
	let serveUp = false
	const server = startServer(config.metricsPort, metrics, {ready: () => consumerRunning && serveUp})

	await zstd.init()
	CompressionCodecs[CompressionTypes.ZSTD] = () => ({
		compress: async (enc: {buf: Uint8Array}) => Buffer.from(zstd.compress(enc.buf)),
		decompress: async (buf: Uint8Array) => Buffer.from(zstd.decompress(buf)),
	})
	const decode: Decode = async (payload) => (await grc.decodeEditAuto(payload)) as unknown as {ops: never[]}

	const store = new Store(`${config.dataDir}/feeder`)
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const publisher = new SpaceMapPublisher(config.spaceMapPath, kubeConfigMapPut(config.spaceMapConfigMap), metrics)
	publisher.start()
	publisher.publish(renderSpaceMap(store.spaces, backfilledSpaces(store.dir)))
	// System Spaces (record families, GEODB_RECORD_KEYS): founded on first write, published under their own key.
	const systemSpaces = new SystemSpaces(`${config.dataDir}/feeder`)
	const systemPublisher = new SpaceMapPublisher(
		config.systemSpaceMapPath,
		kubeConfigMapPut(config.spaceMapConfigMap),
		metrics,
		SYSTEM_SPACE_MAP_KEY,
	)
	systemPublisher.start()
	if (Object.keys(systemSpaces.records).length > 0) systemPublisher.publish(systemSpaces.render())
	log("info", "state loaded", {
		spaces: Object.keys(store.spaces).length,
		entities: store.entityCount,
		statements: store.statementCount,
		hadOffsets: store.hadOffsets,
		topic: config.kafka.topic,
		group: config.kafka.groupId,
	})

	log("info", "waiting for geodb-serve")
	await db.waitReady()
	serveUp = true
	metrics.spaces.set({}, Object.values(store.spaces).filter((s) => s.geo).length)

	const kafka = new Kafka({
		clientId: "geodb-feeder",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	await admin.connect()
	if (!store.hadOffsets) {
		// An empty store must not resume from a group commit left by an earlier store: that would skip edits.
		try {
			await admin.resetOffsets({groupId: config.kafka.groupId, topic: config.kafka.topic, earliest: true})
			log("info", "empty store: group offsets reset to earliest")
		} catch (e) {
			log("warn", "could not reset group offsets (group still active?)", errorFields(e))
		}
	}

	const lock = createLock()
	const spaceDeps = {db, store, metrics, sealer: () => readSealer(config.sealerFile), publish: (t: string) => publisher.publish(t)}
	if (config.spacesListPath && existsSync(config.spacesListPath)) {
		// Found the Spaces already known up front, so the first messages do not wait on a founding.
		for (const g of readFileSync(config.spacesListPath, "utf8").split("\n").map((l) => l.trim()).filter(Boolean)) {
			try {
				await ensureSpace(spaceDeps, g)
			} catch (e) {
				if (!(e instanceof SpaceCapError)) throw e // logged; its edits dead-letter as space_cap
			}
		}
	}

	const consumer = kafka.consumer({
		groupId: config.kafka.groupId,
		sessionTimeout: 120_000,
		heartbeatInterval: 5000,
		maxBytesPerPartition: 4 * 1024 * 1024,
	})
	await consumer.connect()
	await consumer.subscribe({topic: config.kafka.topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		log("error", "consumer crashed", {error: String(e.payload?.error).slice(0, 300)})
		process.exit(1)
	})

	const commits = createGroupCommitter((partition, offset) =>
		consumer.commitOffsets([{topic: config.kafka.topic, partition, offset}]).catch((e) => log("warn", "group commit failed", errorFields(e))),
	)
	await consumer.run({
		autoCommit: false,
		partitionsConsumedConcurrently: 1,
		eachBatch: async ({batch, resolveOffset, heartbeat}) => {
			for (const m of batch.messages) {
				if (abort.signal.aborted) return
				const offset = Number(m.offset)
				const last = store.lastDone(batch.partition)
				if (last !== null && offset <= last) {
					resolveOffset(m.offset)
					await commits.done(batch.partition, offset)
					continue
				}
				if (last !== null && offset > last + 1) metrics.offsetGaps.inc({}, offset - last - 1)
				const deps = {...spaceDeps, lock, beat: heartbeat, signal: abort.signal, decode, chunk: config.chunk, positions: config.relationPositions, entityTimes: config.entityTimes}
				let outcome: string
				for (;;) {
					try {
						outcome = await handleMessage(deps, batch.partition, m)
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				store.markDone(batch.partition, offset)
				metrics.edits.inc({outcome})
				observeFreshness(metrics.freshness, m.timestamp)
				metrics.lastApplied.set({}, Date.now() / 1000)
				metrics.lastOffset.set({partition: String(batch.partition)}, offset)
				resolveOffset(m.offset)
				await heartbeat()
				await commits.done(batch.partition, offset)
			}
			await commits.flush()
		},
	})
	consumerRunning = true
	metrics.consumerUp.set({}, 1)

	const poll = async () => {
		try {
			const s = await db.spaces(10_000)
			serveUp = s !== null
			metrics.serveUp.set({}, serveUp ? 1 : 0)
			for (const p of await admin.fetchTopicOffsets(config.kafka.topic)) {
				const last = store.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				metrics.lag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "lag poll failed", errorFields(e))
		}
	}
	// Membership dual write and shadow comparison: both off unless their flag is set (see membership.ts).
	let membership: Awaited<ReturnType<typeof startMembershipConsumer>> | null = null
	let compareTimer: NodeJS.Timeout | null = null
	let pg: ReadOnlyPg | null = null
	let reader: PipeReader | null = null
	let state: MembershipState | null = null
	if (config.membership.dualWrite || config.membership.shadowCompare) {
		state = new MembershipState(`${config.dataDir}/feeder/membership`)
	}
	// The keyed consumer runs only with the dual write on; the shadow compare follows whichever writer actually runs.
	const keyedMembership = keyedMembershipEnabled(config)
	if (keyedMembership) {
		// Keyed records replace the journal-planned dual write: its own group and offsets, the governance Space.
		membership = await startOptionalConsumer("membership_keyed", metrics.consumerStartFailed, () =>
			startMembershipConsumer({
				kafka,
				config,
				abort,
				keyed: {
					...spaceDeps,
					lock,
					beat: async () => {},
					allowPut: true,
					systemSpaces,
					publishSystem: (t: string) => systemPublisher.publish(t),
					offsets: new ProposalOffsets(keyedOffsetsDir(config.dataDir)),
				},
			}),
		)
	} else if (config.membership.dualWrite && state) {
		try {
			membership = await startMembershipConsumer({
				kafka,
				config,
				abort,
				deps: {...spaceDeps, lock, beat: async () => {}, state},
			})
		} catch (e) {
			log("error", "membership consumer failed to start (edits consumer unaffected)", errorFields(e))
		}
	}
	if (config.membership.shadowCompare && state) {
		const url = process.env.DATABASE_URL
		if (!url) log("error", "GEODB_MEMBERSHIP_SHADOW_COMPARE is set without DATABASE_URL; comparison disabled")
		else {
			const membershipState = state
			pg = new ReadOnlyPg(pgConfig(url, process.env.DATABASE_SSL_CA))
			reader = new PipeReader({clientBin: config.clientBin, keyPath: config.membership.readerKeyPath, bindingPath: config.bindingPath})
			const run = () =>
				compareMembership({
					pgMembers: () => readMembershipIds(pg as ReadOnlyPg),
					reader: reader as PipeReader,
					spaces: () => store.spaces,
					// Keyed rows carry no journal: a Space Postgres has no rows for is not compared.
					journalSpaces: () => (keyedMembership ? [] : membershipState.spaces()),
					metrics,
					...(keyedMembership ? {target: keyedCompareTarget(systemSpaces)} : {}),
				}).catch((e) => log("warn", "membership shadow comparison failed", errorFields(e)))
			compareTimer = setInterval(() => void run(), config.membership.compareIntervalMs)
		}
	}

	let proposals: Awaited<ReturnType<typeof startProposalConsumer>> | null = null
	if (config.proposals.enabled) {
		try {
			proposals = await startProposalConsumer({
				kafka,
				config,
				abort,
				offsets: new ProposalOffsets(`${config.dataDir}/feeder/proposals`),
				deps: {...spaceDeps, lock, beat: async () => {}},
			})
		} catch (e) {
			log("error", "proposals consumer failed to start (edits consumer unaffected)", errorFields(e))
		}
	}

	// Keyed user votes (votesKeyed.ts): GEODB_VOTES with GEODB_RECORD_KEYS, after the votes backfill.
	let votes: Awaited<ReturnType<typeof startVotesConsumer>> | null = null
	if (config.votes.enabled && !config.recordKeys) log("warn", "GEODB_VOTES is set without GEODB_RECORD_KEYS; votes consumer not started")
	if (votesKeyedEnabled(config)) {
		votes = await startOptionalConsumer("votes", metrics.consumerStartFailed, () =>
			startVotesConsumer({
				kafka,
				config,
				abort,
				deps: {
					...spaceDeps,
					lock,
					beat: async () => {},
					allowPut: true,
					systemSpaces,
					publishSystem: (t: string) => systemPublisher.publish(t),
					offsets: new ProposalOffsets(votesOffsetsDir(config.dataDir)),
				},
			}),
		)
	}

	// Keyed proposal votes (proposalVotesKeyed.ts): GEODB_PROPOSAL_VOTES with GEODB_RECORD_KEYS, after their backfill.
	let proposalVotes: Awaited<ReturnType<typeof startProposalVotesConsumer>> | null = null
	if (config.proposalVotes.enabled && !config.recordKeys)
		log("warn", "GEODB_PROPOSAL_VOTES is set without GEODB_RECORD_KEYS; proposal votes consumer not started")
	if (proposalVotesKeyedEnabled(config)) {
		proposalVotes = await startOptionalConsumer("proposal_votes", metrics.consumerStartFailed, () =>
			startProposalVotesConsumer({
				kafka,
				config,
				abort,
				deps: {
					...spaceDeps,
					lock,
					beat: async () => {},
					allowPut: true,
					systemSpaces,
					publishSystem: (t: string) => systemPublisher.publish(t),
					offsets: new ProposalOffsets(proposalVotesOffsetsDir(config.dataDir)),
				},
			}),
		)
	}

	// Keyed proposal actions (proposalActionsKeyed.ts): GEODB_PROPOSAL_ACTIONS with GEODB_RECORD_KEYS, after their backfill.
	let proposalActions: Awaited<ReturnType<typeof startProposalActionsConsumer>> | null = null
	if (config.proposalActions.enabled && !config.recordKeys)
		log("warn", "GEODB_PROPOSAL_ACTIONS is set without GEODB_RECORD_KEYS; proposal actions consumer not started")
	if (proposalActionsKeyedEnabled(config)) {
		proposalActions = await startOptionalConsumer("proposal_actions", metrics.consumerStartFailed, () =>
			startProposalActionsConsumer({
				kafka,
				config,
				abort,
				deps: {
					...spaceDeps,
					lock,
					beat: async () => {},
					allowPut: true,
					systemSpaces,
					publishSystem: (t: string) => systemPublisher.publish(t),
					offsets: new ProposalOffsets(proposalActionsOffsetsDir(config.dataDir)),
				},
			}),
		)
	}

	let spaceEntities: Awaited<ReturnType<typeof startSpaceEntityConsumer>> | null = null
	if (config.spaceEntities.enabled) {
		try {
			const dir = `${config.dataDir}/feeder/space-entities`
			spaceEntities = await startSpaceEntityConsumer({
				kafka,
				config,
				abort,
				offsets: new ProposalOffsets(dir),
				deps: {...spaceDeps, lock, beat: async () => {}, pending: new PendingSpaces(dir)},
			})
		} catch (e) {
			log("error", "space entities consumer failed to start (edits consumer unaffected)", errorFields(e))
		}
	}

	const pollTimer = setInterval(() => void poll(), config.lagPollMs)
	await poll()
	if (membership) {
		const m = membership
		setInterval(() => void m.poll(), config.lagPollMs).unref()
		await m.poll()
	}

	if (proposals) {
		const p = proposals
		setInterval(() => void p.poll(), config.lagPollMs).unref()
		await p.poll()
	}

	if (votes) {
		const v = votes
		setInterval(() => void v.poll(), config.lagPollMs).unref()
		await v.poll()
	}

	if (proposalVotes) {
		const pv = proposalVotes
		setInterval(() => void pv.poll(), config.lagPollMs).unref()
		await pv.poll()
	}

	if (proposalActions) {
		const pa = proposalActions
		setInterval(() => void pa.poll(), config.lagPollMs).unref()
		await pa.poll()
	}

	if (spaceEntities) {
		const s = spaceEntities
		setInterval(() => void s.poll(), config.lagPollMs).unref()
		await s.poll()
	}

	const stop = async () => {
		log("info", "shutting down")
		abort.abort()
		clearInterval(pollTimer)
		if (compareTimer) clearInterval(compareTimer)
		reader?.reset()
		await pg?.close()
		await membership?.stop()
		await proposals?.stop()
		await votes?.stop()
		await proposalVotes?.stop()
		await proposalActions?.stop()
		await spaceEntities?.stop()
		publisher.stop()
		systemPublisher.stop()
		await consumer.disconnect().catch(() => {})
		await admin.disconnect().catch(() => {})
		server.close()
		process.exit(0)
	}
	process.on("SIGTERM", () => void stop())
	process.on("SIGINT", () => void stop())
}

main(loadConfig()).catch((e) => {
	log("error", "fatal", errorFields(e))
	process.exit(1)
})

