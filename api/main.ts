import {Hono} from "hono"
import {cors} from "hono/cors"
import {graphqlServer} from "./src/kg/graphql-entry"

const app = new Hono()
app.use("*", cors())

app.get("/health", (c) => {
	return c.json({healthy: true})
})

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw)
})

export default app
