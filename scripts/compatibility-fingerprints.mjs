import { createHash } from "node:crypto";
const sorted = (value) =>
  Array.isArray(value)
    ? value.map(sorted)
    : value && typeof value === "object"
      ? Object.fromEntries(
          Object.keys(value)
            .sort()
            .map((key) => [key, sorted(value[key])]),
        )
      : value;
// Canonical JSON: recursively sorted keys, compact separators, ASCII escapes.
// The definition matches the original capture script and is platform independent.
export function compatibilityFingerprints(idls) {
  const result = {};
  for (const [name, keys] of Object.entries({
    accountLayout: ["accounts", "types"],
    instruction: ["instructions", "types"],
    event: ["events", "types"],
    error: ["errors"],
    pda: ["instructions"],
  })) {
    const payload = idls.map((idl) =>
      Object.fromEntries(keys.map((key) => [key, idl[key] ?? []])),
    );
    const bytes = JSON.stringify(sorted(payload)).replace(
      /[\u007f-\uffff]/g,
      (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"),
    );
    result[`${name}FingerprintSha256`] = createHash("sha256")
      .update(bytes)
      .digest("hex");
  }
  return result;
}
