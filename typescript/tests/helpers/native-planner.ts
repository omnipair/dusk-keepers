import { execFileSync } from "node:child_process";
import { closeSync, mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** A file supplies an unambiguous EOF for bank fixtures larger than a pipe's
 * capacity, including after LiteSVM has initialized native worker threads. */
export function nativePlan(request: unknown, mode = "--flash-plan"): any {
  const directory = mkdtempSync(join(tmpdir(), "dusk-keeper-plan-"));
  const path = join(directory,"request.json");
  writeFileSync(path,JSON.stringify(request));
  const input = openSync(path,"r");
  try {
    return JSON.parse(execFileSync("target/debug/dusk-keeper",[mode],{
      encoding:"utf8",stdio:[input,"pipe","pipe"],timeout:15_000,
    }));
  } finally { closeSync(input); rmSync(directory,{recursive:true,force:true}); }
}
