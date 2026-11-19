import { local } from "fixture-lib";
if (local !== "local-import-ok") throw new Error("Local dependency unavailable");
console.log(JSON.stringify(JSON.parse(await Bun.stdin.text())));
