import { Hono } from 'hono';
import { Pool } from 'pg';
import { createYoga } from 'graphql-yoga';
import { createPostGraphileSchema, withPostGraphileContext } from 'postgraphile';
import ConnectionFilterPlugin from 'postgraphile-plugin-connection-filter';
import SimplifyInflectionPlugin from '@graphile-contrib/pg-simplify-inflector';

const app = new Hono();

// Create PostgreSQL pool
const pgPool = new Pool({
  connectionString: process.env.DATABASE_URL || 'postgres://user:pass@localhost/mydb',
});

// PostGraphile options
const postgraphileOptions = {
  dynamicJson: true,
  setofFunctionsContainNulls: false,
  ignoreRBAC: false,
  ignoreIndexes: false,
  appendPlugins: [ConnectionFilterPlugin, SimplifyInflectionPlugin],
  disableDefaultMutations: true,
  simpleCollections: "both" as const,
  graphileBuildOptions: {
    connectionFilterRelations: true,
    connectionFilterComputedColumns: true,
    connectionFilterOperatorNames: {
      equalTo: "is",
      equalToInsensitive: "isInsensitive",
      notEqualTo: "isNot",
      notEqualToInsensitive: "isNotInsensitive",
      contains: "in"
    },
    pgOmitListSuffix: true
  },
};

// Create PostGraphile schema
const postgraphileSchema = await createPostGraphileSchema(
  pgPool,
  ['public'],
  postgraphileOptions
);

// Create Yoga instance
const yoga = createYoga({
  schema: postgraphileSchema,
  graphiql: {
    title: 'PostGraphile API',
    defaultQuery: `# Welcome to PostGraphile!
# Your database schema is automatically available as GraphQL
{
  __schema {
    queryType {
      fields {
        name
        description
      }
    }
  }
}`,
  },
  context: async ({ request }) => {

    // Create a promise that will resolve with the PostGraphile context
    const contextPromise = new Promise((resolve) => {
      withPostGraphileContext(
        {
          pgPool,
        },
        async (postgraphileContext) => {
          resolve({
            request,
            ...postgraphileContext,
          });

          // Return a dummy result since withPostGraphileContext expects a result
          // The actual result will be handled by GraphQL execution
          return { data: null };
        }
      );
    });

    return await contextPromise;
  },
});

// Custom handler that ensures proper cleanup
app.all('/graphql', async (c) => {
  try {
    const response = await yoga.fetch(c.req.raw);
    return response;
  } catch (error) {
    console.error('GraphQL execution error:', error);
    return new Response('Internal Server Error', { status: 500 });
  }
});

// Health check
app.get('/health', (c) => {
  return c.json({ status: 'ok' });
});

export default app;
