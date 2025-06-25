import {Graph, type Op} from "@graphprotocol/grc-20"
import rootData from "./25omwWh6HYgeRQKCaSpVpa_ops.json" // 2258 ops
import cryptoData from "./SgjATMbm41LX6naizMqBVd_ops.json" // 22788 ops
import cryptoEventsData from './LHDnAidYUSBJuvq7wDPRQZ_ops.json' // 3701 ops
import regionsData from './D8akqNQr8RMdCdFHecT2n_ops.json'

const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730"
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f"
const CRYPTO_EVENTS_ENTITY_ID = "320ab568-68cf-4587-8dc9-ae82f55587ce"
const REGIONS_ENTITY_ID = "1d7ee87f-70d7-462d-9b72-ce845aa15986"

const deployables = [{
  entityId: ROOT_ENTITY_ID,
  name: "Geo",
  data: rootData.data,
}, {
  entityId: CRYPTO_ENTITY_ID,
  name: "Crypto",
  data: cryptoData.data,
}, {
  entityId: CRYPTO_EVENTS_ENTITY_ID,
  name: "Crypto events",
  data: cryptoEventsData.data,
}, {
  entityId: REGIONS_ENTITY_ID,
  name: "Regions",
  data: regionsData.data
}]

for (const deploy of deployables) {
  console.log(`Deploying ${deploy.name} with ${regionsData.data.length} ops`)

  const space = await Graph.createSpace({
  	editorAddress: "0x84713663033dC5ba5699280728545df11e76BCC1",
  	name: deploy.name,
  	network: "TESTNET",
  	ops: deploy.data as Op[],
  	spaceEntityId: deploy.entityId,
  })

  console.log("space", space)
}

// 55053
// Root 64ed9ffa-e7b3-40f6-ae99-fbf6112d10f8
// Crypto 065d5e6f-0b3c-45b3-a5db-ace191e5a35c
// Crypto news d4cd9afa-3edf-4739-8537-ebb46da159f7
// Regions 1f230cb3-145c-4e4b-b325-e85b0f8a212e
