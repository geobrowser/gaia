This project is a simple script to deploy spaces with seeded data from production.

## How it works

The `main.ts` script deploys Geo spaces by:

1. **Importing space data**: Each space's operations are imported from JSON files (e.g., `root.json`, `crypto.json`) containing the ops that define the space's content
2. **Configuring deployables**: Spaces to deploy are defined in the `deployables` array, where each entry contains:
   - `entityId`: Unique identifier for the space's front page entity
   - `name`: Display name for the space
   - `data`: Reference to the imported JSON ops data
3. **Creating spaces**: For each deployable, the script calls `Graph.createSpace()` with:
   - Editor address (hardcoded wallet address)
   - Space name
   - Network configuration (TESTNET)
   - Ops data from the JSON file
   - Space entity ID
4. **Logging results**: The space ID is logged after successful deployment

## Important notes

- Only uncomment and deploy spaces that haven't been deployed yet to avoid duplicate data
- Previous deployment metadata is automatically saved to `deployed.json` after each successful deployment, keyed by space name with:
  - `spaceId`: The generated space ID
  - `entityId`: The entity ID used for deployment
  - `timestamp`: ISO timestamp of when the deployment occurred- Most spaces are commented out by default - uncomment the ones you want to deploy
- Order matters for deploying if spaces are dependent on properties in other spaces

### Running the script

```sh
# from within /deployer directory
bun install
bun run main.ts
```
