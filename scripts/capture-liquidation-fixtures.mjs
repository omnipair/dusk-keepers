// Reproduce bank fixtures from the pinned protocol's instruction-level tests.
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
const output = resolve("fixtures/litesvm");
await mkdir(output, { recursive: true });
let test = await readFile(`${source}/tests/v2-final-smoke.test.ts`, "utf8");
const capture = `
  let captureIndex=0;
  async function captureKeeperFixture(f:any, position:PublicKey, leverage:boolean, debtAsset:number, action:string, quote:any, maxRepayment:any, instructions:TransactionInstruction[]) {
    const state=accountCoder.decode(leverage?'LeveragePosition':'BorrowPosition',Buffer.from(svm.getAccount(position)!.data)) as any;
    const buyer=Keypair.generate().publicKey;
    const clockKey=new PublicKey('SysvarC1ock11111111111111111111111111111111');
    const keys=new Map<string,PublicKey>();
    const add=(k:any)=>{if(k instanceof PublicKey)keys.set(k.toBase58(),k);};
    Object.values(f).forEach(add);add(position);add(futarchyAuthority);add(payer.publicKey);add(buyer);add(clockKey);
    instructions.forEach(ix=>ix.keys.forEach(k=>add(k.pubkey)));
    const accounts:any={};const bank:any={};
    for(const [key,pubkey] of keys){const a=svm.getAccount(pubkey);accounts[key]=a?{owner:a.owner.toBase58(),data:Array.from(a.data),executable:a.executable}:null;if(a)bank[key]={...accounts[key],lamports:a.lamports.toString()};}
    // A fresh disposable keeper is funded only in the bank fixture. No secret
    // keys are saved; execution tests disable signature verification locally.
    bank[buyer.toBase58()]={owner:SystemProgram.programId.toBase58(),data:[],executable:false,lamports:'10000000000'};
    for(const [mint,original] of [[f.baseMint,f.ownerBaseAccount],[f.quoteMint,f.ownerQuoteAccount]]){
      const token=svm.getAccount(mint)!.owner;
      for(const owner of [buyer,new PublicKey(state.owner)]){
        const ata=getAssociatedTokenAddressSync(mint,owner,true,token);const a=svm.getAccount(original)!;const data=Buffer.from(a.data);owner.toBuffer().copy(data,32);
        const value={owner:token.toBase58(),data:Array.from(data),executable:false};accounts[ata.toBase58()]=value;bank[ata.toBase58()]={...value,lamports:a.lamports.toString()};
      }
    }
    const value={sourceCommit:${JSON.stringify(lock.source.gitCommit)},action,kind:leverage?'LeveragePosition':'BorrowPosition',position:position.toBase58(),buyer:buyer.toBase58(),debtAsset,maxRepayment:maxRepayment.toString(),quoteBase64:program.coder.types.encode(action==='flash'?'flashLiquidationQuote':'emergencyLiquidationQuote',quote).toString('base64'),snapshot:{slot:Number(svm.getClock().slot),accounts},bank,clock:Object.fromEntries(Object.entries(svm.getClock()).map(([k,v])=>[k,String(v)]))};
    fs.writeFileSync(${JSON.stringify(output)}+'/'+String(++captureIndex).padStart(2,'0')+'-'+action+'-'+value.kind+'.json',JSON.stringify(value)+'\\n');
  }
`;
test = test.replace(
  "  async function executeEmergency(",
  capture + "\n  async function executeEmergency(",
);
test = test.replace(
  '    const { computeUnits } = await connection.sendTransactionMeasured(tx, [payer]);\n    return { tx, quote: cpiEvent(tx, "emergencyLiquidationSettled").quote, computeUnits };',
  `    const pq=await program.methods.previewEmergencyLiquidation({debtAsset:1,collateralDebit:slice??(leverage?p.collateral_amount:p.base_collateral),full}).accounts({preview:accounts,futarchyAuthority}).transaction();pq.instructions.unshift(ComputeBudgetProgram.requestHeapFrame({bytes:256*1024}));
    const q=program.coder.types.decode('emergencyLiquidationQuote',await simulateReturnData(pq));
    await captureKeeperFixture(f,position,leverage,1,'emergency',q,new BN(0),tx.instructions);
    const { computeUnits } = await connection.sendTransactionMeasured(tx, [payer]);
    return { tx, quote: cpiEvent(tx, "emergencyLiquidationSettled").quote, computeUnits };`,
);
test = test.replace(
  '    await connection.sendTransaction(tx, [payer]);\n    return { tx, quote, event: cpiEvent(tx, "flashLiquidationSettled") };',
  `    await captureKeeperFixture(f,position,leverage,debtAsset,'flash',quote,new BN(repayment),tx.instructions);
    await connection.sendTransaction(tx, [payer]);
    return { tx, quote, event: cpiEvent(tx, "flashLiquidationSettled") };`,
);
test = test.replace(
  "    const after = (await getAccount(connection as any, f.ownerQuoteAccount)).amount;\n    expect(after > before).to.equal(true);",
  "    const after = (await getAccount(connection as any, f.ownerQuoteAccount)).amount;\n    expect(after > before).to.equal(true);",
);
// The dedicated internal-netting case and concentrated emergency partial.
test = test.replace(
  "    const tx = new Transaction().add(ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }), begin, settle);\n    await connection.sendTransaction(tx, [payer]);",
  '    const tx = new Transaction().add(ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }), begin, settle);\n    await captureKeeperFixture(f,leveragePosition,true,1,"flash",quote,new BN(0),tx.instructions);\n    await connection.sendTransaction(tx, [payer]);',
);
test = test.replace(
  '    const transaction = new Transaction().add(...plan.instructions);\n    await connection.sendTransaction(transaction, [payer]);\n    const quote = cpiEvent(transaction, "emergencyLiquidationSettled").quote;',
  '    const transaction = new Transaction().add(...plan.instructions);\n    await captureKeeperFixture(f,leveragePosition,true,1,"emergency",plan.quote,new BN(0),plan.instructions);\n    await connection.sendTransaction(transaction, [payer]);\n    const quote = cpiEvent(transaction, "emergencyLiquidationSettled").quote;',
);
const file = `${source}/tests/.keeper-capture.test.ts`;
try {
  await writeFile(file, test);
  execFileSync(
    `${source}/node_modules/.bin/mocha`,
    [
      "--require",
      "ts-node/register",
      "--extensions",
      "ts",
      file,
      "--grep",
      "liquidates unhealthy fixed|settles insolvent borrowing immediately|liquidates leverage in the final notice|pays a capped-fee flash|settles a late affordable|concentrated flash fees|flash settlement clears a base-debt|flash debt transfer fees|flash liquidation can repay through Dusk",
    ],
    {
      cwd: source,
      stdio: "inherit",
      env: { ...process.env, DUSK_REQUIRE_COMPLETE_CU_BASELINE: "0" },
    },
  );
} finally {
  await unlink(file);
}
