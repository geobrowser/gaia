import {Graph, type Op} from "@graphprotocol/grc-20"
import rootData from "./25omwWh6HYgeRQKCaSpVpa_ops.json" // 2258 ops
import cryptoData from "./SgjATMbm41LX6naizMqBVd_ops.json" // 22788 ops
import cryptoEventsData from './LHDnAidYUSBJuvq7wDPRQZ_ops.json' // 3701 ops
import regionsData from './D8akqNQr8RMdCdFHecT2n_ops.json'

const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730"
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f"
const CRYPTO_EVENTS_ENTITY_ID = "320ab568-68cf-4587-8dc9-ae82f55587ce"
const REGIONS_ENTITY_ID = "1d7ee87f-70d7-462d-9b72-ce845aa15986"

console.log(`Deploying space with ${regionsData.data.length} ops`)

const space = await Graph.createSpace({
	editorAddress: "0xCA4F46DA82E880C9bDeF0632B32CC82495b661C3",
	name: "Crypto events",
	network: "TESTNET",
	ops: regionsData.data as Op[],
	spaceEntityId: REGIONS_ENTITY_ID,
})

console.log("space", space)

// 54924
// Root 28badf57-306c-4e2f-94da-03d27f16b8d6
// Crypto d630af0d-a8b7-4209-ae46-202517a194ec
// Crypto events 2033ec33-e05d-4763-99f7-0043a671ac4e
// Regions 2e02e5e1-0c1c-4105-b032-a655c375b0e0
