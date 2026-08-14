/**
 * Chain keep-alive.
 *
 * ZeroDev's bundler relayer for chain 55516 has repeatedly wedged after the
 * chain goes idle for a while — its watcher/indexer appears to need a fresh
 * block to resume processing rather than actively polling/retrying on its
 * own. Once wedged, every sponsored UserOperation is accepted (200 OK from
 * eth_sendUserOperation) but never gets a receipt, and stays that way until
 * *some* new block appears on-chain — by any means, not necessarily one the
 * bundler itself produced.
 *
 * Scanning the chain's full history (block 1 → current tip) found this has
 * happened repeatedly since June, with idle gaps ranging from ~11 hours to
 * 18 days. This job runs on a short, fixed schedule and — only if the chain
 * has actually gone quiet for a while — sends a trivial, real-gas
 * (non-sponsored) self-transfer directly to the chain, bypassing the
 * ZeroDev bundler entirely. That's deliberate: a keep-alive that itself
 * depends on the thing that's wedging can't unwedge it.
 *
 * Background: geobrowser/geogenesis GEO-2549 / GEO-2550.
 */
import {createPublicClient, createWalletClient, defineChain, http, parseGwei} from "viem"
import {privateKeyToAccount} from "viem/accounts"

/**
 * If the latest block is older than this, the chain is considered idle and
 * worth nudging. Comfortably under the shortest wedge-inducing gap observed
 * historically (~11h), so this intervenes well before the bundler has a
 * chance to wedge rather than reacting after the fact.
 */
const DEFAULT_IDLE_THRESHOLD_MS = 10 * 60 * 1000

/**
 * Floor for the keep-alive tx's own fee, independent of the chain's current
 * fee estimate. The chain's advisory `eth_maxPriorityFeePerGas` returns 0
 * and the base fee has been as low as 0.01 gwei — technically enough by
 * itself, but this job's only purpose is to reliably unwedge things, and gas
 * is effectively free on this testnet. 1 gwei is the exact value verified
 * (manually, during the Aug 2026 incident) to mine instantly; never go
 * below it even if the dynamic estimate comes back lower.
 */
const MIN_FEE_PER_GAS = parseGwei("1")

export function formatMinutes(ms: number): string {
	return (ms / 60_000).toFixed(1)
}

/** How long the chain has been idle, given the latest block's timestamp (seconds, as returned by RPC). */
export function idleMsSince(latestBlockTimestampSeconds: bigint, nowMs: number): number {
	return nowMs - Number(latestBlockTimestampSeconds) * 1000
}

/** Whether idleMs has crossed the threshold that makes a keep-alive send worthwhile. */
export function shouldSendKeepAlive(idleMs: number, idleThresholdMs: number): boolean {
	return idleMs >= idleThresholdMs
}

/**
 * Floors a fee estimate at `floor`, since gas is effectively free on this
 * testnet and this job's only purpose is reliably unwedging the chain — see
 * MIN_FEE_PER_GAS's doc comment.
 */
export function withFeeFloor(estimated: bigint, floor: bigint): bigint {
	const doubled = estimated * 2n
	return doubled > floor ? doubled : floor
}

async function main() {
	const rpcUrl = process.env.RPC_URL
	const privateKey = process.env.KEEPALIVE_PRIVATE_KEY
	const chainId = Number(process.env.CHAIN_ID ?? "55516")
	const idleThresholdMs = Number(process.env.IDLE_THRESHOLD_MS ?? String(DEFAULT_IDLE_THRESHOLD_MS))

	if (!rpcUrl) throw new Error("RPC_URL is required")
	if (!privateKey) throw new Error("KEEPALIVE_PRIVATE_KEY is required")

	const chain = defineChain({
		id: chainId,
		name: "geo-testnet",
		nativeCurrency: {name: "Geo", symbol: "GEO", decimals: 18},
		rpcUrls: {default: {http: [rpcUrl]}},
	})

	const account = privateKeyToAccount(privateKey as `0x${string}`)
	const publicClient = createPublicClient({chain, transport: http(rpcUrl)})
	const walletClient = createWalletClient({account, chain, transport: http(rpcUrl)})

	const latest = await publicClient.getBlock({blockTag: "latest"})
	const idleMs = idleMsSince(latest.timestamp, Date.now())

	console.log(
		`Latest block #${latest.number} is ${formatMinutes(idleMs)} min old (threshold: ${formatMinutes(idleThresholdMs)} min)`,
	)

	if (!shouldSendKeepAlive(idleMs, idleThresholdMs)) {
		console.log("Chain has recent activity — nothing to do")
		return
	}

	console.log(`Chain idle for ${formatMinutes(idleMs)} min — sending keep-alive transaction`)

	const estimate = await publicClient.estimateFeesPerGas({chain, type: "eip1559"})
	const hash = await walletClient.sendTransaction({
		to: account.address,
		value: 0n,
		maxFeePerGas: withFeeFloor(estimate.maxFeePerGas, MIN_FEE_PER_GAS),
		maxPriorityFeePerGas: withFeeFloor(estimate.maxPriorityFeePerGas, MIN_FEE_PER_GAS),
	})

	console.log(`Submitted ${hash} — waiting for confirmation`)
	const receipt = await publicClient.waitForTransactionReceipt({hash, timeout: 60_000})

	if (receipt.status !== "success") {
		throw new Error(`Keep-alive transaction reverted: ${hash}`)
	}

	console.log(`Confirmed in block #${receipt.blockNumber}`)
}

// Guards against side effects when this module is imported for its pure
// helpers (see tests/index.test.ts) rather than run as the CronJob entrypoint.
if (import.meta.main) {
	await main()
}
