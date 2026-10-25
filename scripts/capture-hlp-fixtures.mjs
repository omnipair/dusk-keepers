// Reproduce hLP recovery bank fixtures from the pinned protocol tests.
// Writes an ignored temporary test file; never modifies tracked protocol files.
import { readFile, writeFile, unlink, mkdir } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

const source = resolve(process.argv[2] ?? "");
const lock = JSON.parse(await readFile("protocol.lock.json", "utf8"));
if (
  execFileSync("git", ["-C", source, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim() !== lock.source.gitCommit
)
  throw new Error("protocol source differs from pin");

const output = resolve("fixtures/hlp-litesvm");
await mkdir(output, { recursive: true });
let test = await readFile(`${source}/tests/v2-final-smoke.test.ts`, "utf8");
const capture = String.raw`
  let keeperHlpCaptureIndex = 0;
  async function captureKeeperHlpFixture(
    f: any,
    target: "base" | "quote",
    action: "hlp_flash" | "hlp_close",
    quote: any,
    instructions: TransactionInstruction[],
  ) {
    const buyer = Keypair.generate().publicKey;
    const clockKey = new PublicKey("SysvarC1ock11111111111111111111111111111111");
    const keys = new Map<string, PublicKey>();
    const add = (key: any) => {
      if (key instanceof PublicKey) keys.set(key.toBase58(), key);
    };
    Object.values(f).forEach(add);
    add(f.market); add(futarchyAuthority); add(payer.publicKey); add(buyer); add(clockKey);
    instructions.forEach((ix) => {
      add(ix.programId);
      ix.keys.forEach((key) => add(key.pubkey));
    });
    const accounts: any = {};
    const bank: any = {};
    for (const [key, pubkey] of keys) {
      const account = svm.getAccount(pubkey);
      accounts[key] = account ? {
        owner: account.owner.toBase58(),
        data: Array.from(account.data),
        executable: account.executable,
      } : null;
      if (account) bank[key] = { ...accounts[key], lamports: account.lamports.toString() };
    }
    bank[buyer.toBase58()] = {
      owner: SystemProgram.programId.toBase58(), data: [], executable: false,
      lamports: "10000000000",
    };
    for (const [mint, original] of [
      [f.baseMint, f.ownerBaseAccount],
      [f.quoteMint, f.ownerQuoteAccount],
    ]) {
      const token = svm.getAccount(mint)!.owner;
      const ata = getAssociatedTokenAddressSync(mint, buyer, true, token);
      const originalAccount = svm.getAccount(original)!;
      const data = Buffer.from(originalAccount.data);
      buyer.toBuffer().copy(data, 32);
      const value = { owner: token.toBase58(), data: Array.from(data), executable: false };
      accounts[ata.toBase58()] = value;
      bank[ata.toBase58()] = { ...value, lamports: originalAccount.lamports.toString() };
    }
    const value = {
      sourceCommit: ${JSON.stringify(lock.source.gitCommit)},
      action,
      kind: "Market",
      position: f.market.toBase58(),
      market: f.market.toBase58(),
      buyer: buyer.toBase58(),
      targetAsset: target === "base" ? 0 : 1,
      quoteBase64: quote
        ? program.coder.types.encode("hlpFlashQuote", quote).toString("base64")
        : "",
      maxInsuranceDraw: "0",
      maxSocializedLoss: "18446744073709551615",
      minCallerBountyOut: "1",
      snapshot: { slot: Number(svm.getClock().slot), accounts },
      bank,
      clock: Object.fromEntries(
        Object.entries(svm.getClock()).map(([key, entry]) => [key, String(entry)]),
      ),
    };
    fs.writeFileSync(
      ${JSON.stringify(output)} + "/" + String(++keeperHlpCaptureIndex).padStart(2, "0") + "-" + action + ".json",
      JSON.stringify(value) + "\n",
    );
  }
  function keeperHlpCaptureScenario(fees: boolean, amp: number, target: string, mode: string) {
    return !fees && amp === 1 && target === "base" && (mode === "inventory" || mode === "full");
  }
`;

const marker = '  it("settles vault-wide hLP flash recovery atomically with SDK quotes and fee tokens", async function () {';
if (!test.includes(marker)) throw new Error("hLP flash test marker changed");
test = test.replace(marker, capture + "\n" + marker);

const loopMarker = '    for (const fees of [false, true]) for (const amp of [1, 4]) for (const target of ["base", "quote"] as const) for (const mode of ["inventory", "amm", "full"]) {';
if (!test.includes(loopMarker)) throw new Error("hLP scenario loop changed");
test = test.replace(
  loopMarker,
  loopMarker + '\n      if (!keeperHlpCaptureScenario(fees, amp, target, mode)) continue;',
);

const goodMarker = '      let simulation: any;';
if (!test.includes(goodMarker)) throw new Error("hLP good-plan marker changed");
test = test.replace(
  goodMarker,
  `      if (!fees && amp === 1 && target === "base" && (mode === "inventory" || mode === "full")) {
        await captureKeeperHlpFixture(f, target, "hlp_flash", good.quote, good.instructions);
      }
${goodMarker}`,
);

const closeMarker = '    await connection.sendTransaction(tx, [payer]);\n    trackV2Instruction("closeInsolventHlp", this.test?.title);';
if (!test.includes(closeMarker)) throw new Error("hLP terminal-close marker changed");
test = test.replace(
  closeMarker,
  '    await captureKeeperHlpFixture(fixture, "base", "hlp_close", null, tx.instructions);\n' + closeMarker,
);

const file = `${source}/tests/.keeper-hlp-capture.test.ts`;
try {
  await writeFile(file, test);
  execFileSync(
    `${source}/node_modules/.bin/mocha`,
    [
      "--no-config", "--node-option", "loader=ts-node/esm", "--require", "ts-node/register",
      "--extensions", "ts", file, "--grep",
      "settles vault-wide hLP flash recovery|closes terminal hLP",
    ],
    {
      cwd: source,
      stdio: "inherit",
      env: { ...process.env, DUSK_REQUIRE_COMPLETE_CU_BASELINE: "0" },
    },
  );
} finally {
  await unlink(file).catch((error) => {
    if (error?.code !== "ENOENT") throw error;
  });
}
