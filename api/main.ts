import {Effect, Either, Layer} from "effect"
import {Hono} from "hono"
import {cors} from "hono/cors"
import {graphqlServer} from "./src/kg/graphql-entry"
import {Environment, make as makeEnvironment} from "./src/services/environment"
import {uploadEdit, uploadFile} from "./src/services/ipfs"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)

const app = new Hono()
app.use("*", cors())

app.get("/health", (c) => {
	return c.json({healthy: true})
})

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw)
})

app.post("/ipfs/upload-edit", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const result = await Effect.runPromise(Effect.either(uploadEdit(file)).pipe(Effect.provide(EnvironmentLayer)))

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

app.post("/ipfs/upload-file", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const result = await Effect.runPromise(Effect.either(uploadFile(file)).pipe(Effect.provide(EnvironmentLayer)))

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

export default app
