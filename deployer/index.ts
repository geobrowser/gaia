import {Graph, type Op} from "@graphprotocol/grc-20"
import rootData from "./25omwWh6HYgeRQKCaSpVpa_ops.json" // 2258 ops
import cryptoData from "./SgjATMbm41LX6naizMqBVd_ops.json" // 22788 ops
import cryptoEventsData from './LHDnAidYUSBJuvq7wDPRQZ_ops.json' // 3701 ops
import regionsData from './D8akqNQr8RMdCdFHecT2n_ops.json'

const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730"
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f"
const CRYPTO_EVENTS_ENTITY_ID = "26c7edb1-ca79-4a56-8cc6-04a8e689a8f0"
const REGIONS_ENTITY_ID = "c0814872-672e-46c0-8cdb-73ad34e0b77b"

console.log(`Deploying space with ${regionsData.data.length} ops`)

const space = await Graph.createSpace({
	editorAddress: "0xCA4F46DA82E880C9bDeF0632B32CC82495b661C3",
	name: "Regions",
	network: "TESTNET",
	ops: regionsData.data as Op[],
	spaceEntityId: REGIONS_ENTITY_ID,
})

console.log("space", space)

// Root b02b4b5f-5082-4b3f-a2b5-1d6ca3fa7fbc
// Crypto f1e17dc1-a6c5-4005-9765-5640c4b1f68f
// Crypto events a671acd2-a1e2-4224-bb49-236030c1ec2d
// Regions 6060f4b8-01ce-4735-ac56-4b4940d4b9c5
