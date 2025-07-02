import express from "express";
import { postgraphile } from "postgraphile";
import ConnectionFilterPlugin from 'postgraphile-plugin-connection-filter';

const middleware = postgraphile(process.env.DATABASE_URL!, "public", {
	appendPlugins: [ConnectionFilterPlugin],
	graphiql: true,
	enhanceGraphiql: true,
	disableQueryLog: true,
	simpleCollections: "both",
	// Enable function-based computed columns
	setofFunctionsContainNulls: false,
	// Additional performance optimizations
	enableQueryBatching: true,
	// Debugging options
	showErrorStack: true,
	extendedErrors: ['hint', 'detail', 'errcode'],
	allowExplain: true,
	// Additional debugging
	watchPg: true,
	dynamicJson: true,
	ignoreRBAC: false,
	graphileBuildOptions: {
    connectionFilterRelations: true, // default: false
  },
});

const app = express();

app.use(middleware);

const server = app.listen(5678, () => {
	const address = server.address();
	if (typeof address !== "string") {
		const href = `http://localhost:${address?.port}/graphiql`;
		console.log(`PostGraphiQL available at ${href} 🚀`);
	} else {
		console.log(`PostGraphile listening on ${address} 🚀`);
	}
});
