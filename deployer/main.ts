import { Graph, type Op } from "@graphprotocol/grc-20";
import { existsSync, readFileSync, writeFileSync } from "fs";
import academiaData from "./data/academia.json";
import cryptoData from "./data/crypto.json"; // 22788 ops
import cryptoEventsData from "./data/crypto-events.json"; // 3701 ops
import educationData from "./data/education.json";
// import cryptoNewsData from './data/crypto-news.json';
import industriesData from "./data/industries.json";
import regionsData from "./data/regions.json";
import rootData from "./data/root.json"; // 2258 ops
import sfData from "./data/san-francisco.json";
import technologyData from "./data/technology.json";

const DUMMY_ENTITY_ID = "3beec887-060d-4d55-9710-863f9206fddf";
const ROOT_ENTITY_ID = "6b9f649e-38b6-4224-927d-d66171343730";
const CRYPTO_ENTITY_ID = "23575692-bda8-4a71-8694-04da2e2af18f";
const CRYPTO_EVENTS_ENTITY_ID = "320ab568-68cf-4587-8dc9-ae82f55587ce";
const REGIONS_ENTITY_ID = "1d7ee87f-70d7-462d-9b72-ce845aa15986";
const CRYPTO_NEWS_ENTITY_ID = "fd34c360-59ca-4284-9f13-44d81da56837";
const INDUSTRIES_ENTITY_ID = "51725d6a-21b2-4396-a89c-1b7d2008ac65";
const EDUCATION_ENTITY_ID = "be259c52-532d-4269-9a6c-e93dd4f11e17";
const ACADEMIA_ENTITY_ID = "0fa96f99-1faa-48f2-b825-a1113de0e4be";
const TECHNOLOGY_ENTITY_ID = "a56e7d86-9a0a-47df-a1f9-6c2cf658f79e";
const SF_ENTITY_ID = "16faead7-86d6-4579-b7ea-e43cbdb2db05";

type Deployable = {
	entityId: string;
	name: string;
	data: Op[];
};

/**
 * @NOTE Only deploy spaces that haven't already been deployed or you'll end up duplicate data.
 * See deployed.json for metadata for each already-deployed space.
 */
const deployables: Deployable[] = [
	// {
	// 	entityId: ROOT_ENTITY_ID,
	// 	name: "Geo",
	// 	data: rootData,
	// },
	{
		entityId: CRYPTO_ENTITY_ID,
		name: "Crypto",
		data: cryptoData,
	},
	{
		entityId: CRYPTO_EVENTS_ENTITY_ID,
		name: "Crypto events",
		data: cryptoEventsData,
	},
	{
		entityId: REGIONS_ENTITY_ID,
		name: "Regions",
		data: regionsData,
	},

	{
		entityId: INDUSTRIES_ENTITY_ID,
		name: "Industries",
		data: industriesData,
	},
	{
		entityId: EDUCATION_ENTITY_ID,
		name: "Education",
		data: educationData,
	},
	{
		entityId: ACADEMIA_ENTITY_ID,
		name: "Academia",
		data: academiaData,
	},
	{
		entityId: TECHNOLOGY_ENTITY_ID,
		name: "Technology",
		data: technologyData,
	},
	{
		entityId: SF_ENTITY_ID,
		name: "San Francisco",
		data: sfData,
	},
];

// Load existing deployed spaces or create new object
let deployed: Record<
	string,
	{ spaceId: string; entityId: string; timestamp: string }
> = {};
const deployedFilePath = "./deployed.json";

if (existsSync(deployedFilePath)) {
	const existingData = readFileSync(deployedFilePath, "utf8");
	deployed = JSON.parse(existingData);
}

for (const deploy of deployables) {
	console.log(`Deploying ${deploy.name} with ${deploy.data.length} ops`);

	const space = await Graph.createSpace({
		editorAddress: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
		name: deploy.name,
		network: "TESTNET",
		ops: deploy.data as Op[],
		spaceEntityId: deploy.entityId,
	});

	console.log(`${deploy.name} spaceId: `, space);

	// Record deployment info
	deployed[deploy.name] = {
		spaceId: space.id,
		entityId: deploy.entityId,
		timestamp: new Date().toISOString(),
	};

	// Write to deployed.json
	writeFileSync(deployedFilePath, JSON.stringify(deployed, null, 2));
	console.log(`Saved deployment info to ${deployedFilePath}`);
}
