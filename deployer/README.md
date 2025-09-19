This project is a simple script to deploy spaces with seeded data from production.

`main.ts` contains the following

- imports with the JSON representation of the ops for each space
- array containing list of spaces to deploy when running the script
- entity ids representing each space's front page
- logging of the space id when each space is deployed

### Running the script

```sh
# from within /deployer directory
bun install
bun run main.ts
```
