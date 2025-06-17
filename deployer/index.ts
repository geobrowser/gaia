import {Graph, type Op} from "@graphprotocol/grc-20"
import {privateKeyToAccount} from "viem/accounts"
import rootData from "./25omwWh6HYgeRQKCaSpVpa_ops.json" // 2195 ops
import cryptoData from "./SgjATMbm41LX6naizMqBVd_ops.json" // 22751 ops

const PK = process.env.PK

if (!PK) {
	throw new Error("PK must exist in environment")
}

const {address} = privateKeyToAccount(PK as `0x${string}`)

const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730"
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f"

console.log(`Deploying space with ${rootData.data.length} ops`)

const space = await Graph.createSpace({
	editorAddress: address,
	name: "Test root",
	network: "TESTNET",
	ops: rootData.data as Op[],
	spaceEntityId: ROOT_ENTITY_ID,
})

console.log("space", space)
