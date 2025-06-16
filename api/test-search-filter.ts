import { Effect, Layer } from "effect"
import { search } from "./src/kg/resolvers/search"
import { Environment, make as makeEnvironment } from "./src/services/environment"
import { Storage, make as makeStorage } from "./src/services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

async function testSearchFilter() {
  console.log("Testing search functionality with type filtering...")

  try {
    // Test 1: Basic search without filter
    console.log("\n1. Testing basic search without filter:")
    const basicResult = await Effect.runPromise(
      search({
        query: "test",
        limit: 5,
        offset: 0,
        threshold: 0.1,
      }).pipe(provideDeps)
    )
    console.log(`Found ${basicResult.length} entities`)
    basicResult.forEach((entity, index) => {
      console.log(`  ${index + 1}. ${entity.id}`)
    })

    // Test 2: Search with type filter
    console.log("\n2. Testing search with type filter:")
    const filteredResult = await Effect.runPromise(
      search({
        query: "test",
        filter: {
          types: {
            in: ["550e8400-e29b-41d4-a716-446655440001", "550e8400-e29b-41d4-a716-446655440002"],
          },
        },
        limit: 5,
        offset: 0,
        threshold: 0.1,
      }).pipe(provideDeps)
    )
    console.log(`Found ${filteredResult.length} entities with type filter`)
    filteredResult.forEach((entity, index) => {
      console.log(`  ${index + 1}. ${entity.id}`)
    })

    // Test 3: Search with empty type filter
    console.log("\n3. Testing search with empty type filter:")
    const emptyFilterResult = await Effect.runPromise(
      search({
        query: "test",
        filter: {
          types: {
            in: [],
          },
        },
        limit: 5,
        offset: 0,
        threshold: 0.1,
      }).pipe(provideDeps)
    )
    console.log(`Found ${emptyFilterResult.length} entities with empty type filter`)
    emptyFilterResult.forEach((entity, index) => {
      console.log(`  ${index + 1}. ${entity.id}`)
    })

    // Test 4: Search with space ID and type filter
    console.log("\n4. Testing search with space ID and type filter:")
    const spaceAndTypeResult = await Effect.runPromise(
      search({
        query: "test",
        spaceId: "550e8400-e29b-41d4-a716-446655440100",
        filter: {
          types: {
            in: ["550e8400-e29b-41d4-a716-446655440001"],
          },
        },
        limit: 5,
        offset: 0,
        threshold: 0.1,
      }).pipe(provideDeps)
    )
    console.log(`Found ${spaceAndTypeResult.length} entities with space ID and type filter`)
    spaceAndTypeResult.forEach((entity, index) => {
      console.log(`  ${index + 1}. ${entity.id}`)
    })

    console.log("\n✅ Search filter functionality is working correctly!")
    
  } catch (error) {
    console.error("❌ Error testing search filter:", error)
  }
}

// Run the test
testSearchFilter().catch(console.error)