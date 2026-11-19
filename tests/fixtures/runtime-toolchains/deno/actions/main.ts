import { local } from "../lib/helper.ts";
if (local !== "local-import-ok") throw new Error("Local import unavailable");
console.log(JSON.stringify(JSON.parse(await new Response(Deno.stdin.readable).text())));
