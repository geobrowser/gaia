import {Graph, type Op} from "@graphprotocol/grc-20"
import rootData from "./root.json" // 2258 ops
import cryptoData from "./crypto.json" // 22788 ops
import cryptoEventsData from './crypto-events.json' // 3701 ops
import regionsData from './regions.json'
import cryptoNewsData from './crypto-news.json';
import sfData from './sf.json'
import industriesData from './industries.json'
import educationData from './education.json'
import academiaData from './academia.json'
import technologyData from './technology.json'

const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730"
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f"
const CRYPTO_EVENTS_ENTITY_ID = "320ab568-68cf-4587-8dc9-ae82f55587ce"
const REGIONS_ENTITY_ID = "1d7ee87f-70d7-462d-9b72-ce845aa15986"
const CRYPTO_NEWS_ENTITY_ID = "fd34c360-59ca-4284-9f13-44d81da56837"
const SF_ENTITY_ID = "16faead7-86d6-4579-b7ea-e43cbdb2db05"
const INDUSTRIES_ENTITY_ID = "51725d6a-21b2-4396-a89c-1b7d2008ac65"
const EDUCATION_ENTITY_ID = "be259c52-532d-4269-9a6c-e93dd4f11e17"
const ACADEMIA_ENTITY_ID = "0fa96f99-1faa-48f2-b825-a1113de0e4be"
const TECHNOLOGY_ENTITY_ID = "a56e7d86-9a0a-47df-a1f9-6c2cf658f79e"

const deployables = [
//   {
//   entityId: ROOT_ENTITY_ID,
//   name: "Geo",
//   data: rootData,
// },
// {
//   entityId: CRYPTO_ENTITY_ID,
//   name: "Crypto",
//   data: cryptoData,
// },
// {
//   entityId: CRYPTO_EVENTS_ENTITY_ID,
//   name: "Crypto events",
//   data: cryptoEventsData,
// }, {
//   entityId: REGIONS_ENTITY_ID,
//   name: "Regions",
//   data: regionsData
// },
// {
//   entityId: CRYPTO_NEWS_ENTITY_ID,
//   name: "Crypto news",
//   data: cryptoNewsData
// },
{
  entityId: SF_ENTITY_ID,
  name: "San Francisco",
  data: sfData
},
  // {
  //   entityId: INDUSTRIES_ENTITY_ID,
  //   name: "Industries",
  //   data: industriesData
  // },
  // {
  //   entityId: EDUCATION_ENTITY_ID,
  //   name: "Education",
  //   data: educationData
  // },
  // {
  //   entityId: ACADEMIA_ENTITY_ID,
  //   name: "Academia",
  //   data: academiaData
  // },
  // {
  //   entityId: TECHNOLOGY_ENTITY_ID,
  //   name: "Technology",
  //   data: technologyData
  // }
]

for (const deploy of deployables) {
  console.log(`Deploying ${deploy.name} with ${deploy.data.length} ops`)

  const space = await Graph.createSpace({
  	editorAddress: "0x84713663033dC5ba5699280728545df11e76BCC1",
  	name: deploy.name,
  	network: "TESTNET",
  	ops: deploy.data as Op[],
  	spaceEntityId: deploy.entityId,
  })

  console.log("space", space)
}

// 56013
// Root 2df11968-9d1c-489f-91b7-bdc88b472161
// Crypto b2565802-3118-47be-91f2-e59170735bac
// Crypto events dabe3133-4334-47a0-85c5-f965a3a94d4c
// Regions aea9f05a-2797-4e7e-aeae-5059ada3b56b
// Crypto news 27af9116-ddb6-4baa-b4f0-c54f0774d346
// Industries bf44e948-07e0-4297-a2e7-371c22670f98
// Education 0fcbc499-71e5-4505-a081-aa26edd97937
// Academia b9192469-f28f-498c-a486-b78f68ab05f0
// Technology d1a3b7e7-37c0-4126-b2bf-53ff46977fb2
// SF b42fa1af-1d67-4058-a6f1-4be5d7360caf
