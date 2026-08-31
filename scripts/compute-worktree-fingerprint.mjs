/**
 * Compute the source fingerprint recorded in `protocol.lock.json`.
 *
 * The lock pins binary hashes, which say what was deployed, and a git commit,
 * which says what was *supposed* to have been deployed. Neither covers the
 * common and dangerous case: a binary built from a working tree that carried
 * changes the commit does not. This fingerprint covers exactly that tree.
 *
 * A fingerprint nobody can recompute is decoration, so this is a script rather
 * than a constant somebody once pasted in. The definition is deliberately
 * simple: every tracked file under the program and SDK source directories,
 * sorted by path, hashed as `path\0sha256(content)\n`. Sorting makes it
 * independent of filesystem order; hashing content rather than mtimes makes a
 * checkout of the same tree produce the same answer.
 *
 *   node scripts/compute-worktree-fingerprint.mjs /path/to/dusk [--write]
 */

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";

/**
 * The directories whose contents decide what the deployed programs do.
 *
 * The SDK is deliberately excluded. It is a client, hashed separately as
 * `sdk.sha256`, and folding it in here would make the deployment's source
 * attestation change every time a builder was added to a library that is not
 * part of the binary.
 */
const TRACKED_PREFIXES = ["programs/"];

async function fingerprint(root) {
  const tracked = execFileSync("git", ["-C", root, "ls-files", "-z"], {
    maxBuffer: 64 * 1024 * 1024,
  })
    .toString("utf8")
    .split("\0")
    .filter((path) => path && TRACKED_PREFIXES.some((prefix) => path.startsWith(prefix)))
    .sort();

  if (tracked.length === 0) {
    throw new Error(`${root}: no tracked source files matched ${TRACKED_PREFIXES.join(", ")}`);
  }

  const digest = createHash("sha256");
  for (const path of tracked) {
    const content = await readFile(`${root}/${path}`);
    digest.update(path);
    digest.update("\0");
    digest.update(createHash("sha256").update(content).digest("hex"));
    digest.update("\n");
  }
  return { fingerprint: digest.digest("hex"), files: tracked.length };
}

const root = process.argv[2];
if (!root) {
  console.error("usage: compute-worktree-fingerprint.mjs <dusk-repo-path> [--write]");
  process.exit(1);
}

const { fingerprint: value, files } = await fingerprint(root);
console.log(`${value}  (${files} tracked source files under ${TRACKED_PREFIXES.join(", ")})`);

if (process.argv.includes("--write")) {
  const lockPath = new URL("../protocol.lock.json", import.meta.url);
  const lock = JSON.parse(await readFile(lockPath, "utf8"));
  lock.source.worktreeFingerprintSha256 = value;
  await writeFile(lockPath, `${JSON.stringify(lock, null, 2)}\n`);
  console.log("wrote source.worktreeFingerprintSha256");
}
