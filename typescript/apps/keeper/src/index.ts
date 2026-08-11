import { createServer } from "node:http";
import { assertLiveReady, readProtocolLock } from "../../../packages/dusk-adapter/src/index.ts";

type Mode = "shadow" | "live";

function parseMode(value: string): Mode {
  if (value === "shadow" || value === "live") return value;
  throw new Error(`KEEPER_MODE must be shadow or live, got ${value}`);
}

const mode = parseMode(process.env.KEEPER_MODE ?? "shadow");
const profile = process.env.KEEPER_PROFILE ?? "conformance";
const lock = await readProtocolLock(process.env.DUSK_PROTOCOL_LOCK ?? "protocol.lock.json");

if (mode === "live") {
  assertLiveReady(lock);
  throw new Error("live executor is intentionally not implemented in this scaffold");
}

console.log(`keeper profile=${profile} mode=shadow protocol_revision=${lock.revision}`);
if (process.argv.includes("--check")) process.exit(0);

const port = Number.parseInt(process.env.PORT ?? "8080", 10);
const server = createServer((request, response) => {
  if (request.url !== "/healthz" && request.url !== "/readyz") {
    response.writeHead(404, { "content-type": "application/json" });
    response.end(JSON.stringify({ status: "not_found" }));
    return;
  }
  response.writeHead(200, { "content-type": "application/json" });
  response.end(
    JSON.stringify({
      status: "healthy",
      mode,
      profile,
      protocolRevision: lock.revision,
    }),
  );
});

server.listen(port, "0.0.0.0");

