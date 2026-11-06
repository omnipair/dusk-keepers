// Replay captured native keeper plans on an isolated offline Surfpool instance.
import assert from 'node:assert/strict';
import { readFileSync,readdirSync,writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { PublicKey,VersionedTransaction } from '@solana/web3.js';
import { nativePlan } from '../typescript/tests/helpers/native-planner.ts';
import { payment } from '../typescript/tests/helpers/liquidation-bank.ts';
import { decodeLiquidationQuote } from '../typescript/packages/dusk-adapter/src/liquidation.ts';
const endpoint=process.env.SURFPOOL_REHEARSAL_RPC??'http://127.0.0.1:18899';
if(!['127.0.0.1','localhost','::1'].includes(new URL(endpoint).hostname))throw Error('Rehearsal requires a local RPC');
let requestId=0;
async function rpc(method,params=[]){const res=await fetch(endpoint,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:++requestId,method,params}),signal:AbortSignal.timeout(25000)});const value=await res.json();if(value.error)throw Error(method+': '+JSON.stringify(value.error));return value.result;}
async function setAccount(address,account){await rpc('surfnet_setAccount',[address,{owner:account.owner,executable:account.executable,lamports:Number(account.lamports??10000000),data:Buffer.from(account.data).toString('hex')}]);}
const lock=JSON.parse(readFileSync('protocol.lock.json'));
for(const program of lock.programs){
 const bytes=readFileSync(program.binary.path);
 assert.equal(createHash('sha256').update(bytes).digest('hex'),program.binary.sha256,'pinned executable hash');
 await rpc('surfnet_writeProgram',[program.programId,bytes.toString('hex'),0]);
}
const rows=[];
for(const directory of ['fixtures/litesvm','fixtures/hlp-litesvm'])for(const file of readdirSync(directory).filter(f=>f.endsWith('.json')).sort()){
 const fixture=JSON.parse(readFileSync(directory+'/'+file));
 const clock=Buffer.from(fixture.bank['SysvarC1ock11111111111111111111111111111111'].data);
 const slot=Number(clock.readBigUInt64LE());
 const current=await rpc('getSlot',[{commitment:'processed'}]);await rpc('surfnet_timeTravel',[{absoluteSlot:Math.max(slot,current+100)}]);
 for(const [address,account]of Object.entries(fixture.bank))if(!account.executable)await setAccount(address,account);
 const quote=fixture.action==='hlp_close'?null:fixture.action.startsWith('hlp')?null:decodeLiquidationQuote(fixture.action,Buffer.from(fixture.quoteBase64,'base64'));
 const request=fixture.action==='flash'?{...fixture,internal:false,route:[payment(fixture,quote)]}:fixture;
 let plan=nativePlan(request);
 const addresses=[...new Set(plan.instructions.flatMap(ix=>ix.accounts.filter(meta=>!meta.is_signer).map(meta=>new PublicKey(meta.pubkey).toBase58())))];
 const lookupTable=new PublicKey(createHash('sha256').update(file+'surfpool').digest()).toBase58();
 const lookup=Buffer.alloc(56+addresses.length*32);lookup.writeUInt32LE(1);lookup.writeBigUInt64LE(0xffffffffffffffffn,4);addresses.forEach((address,index)=>new PublicKey(address).toBuffer().copy(lookup,56+index*32));
 await setAccount(lookupTable,{owner:'AddressLookupTab1e1111111111111111111111111',executable:false,lamports:10000000,data:lookup});
 plan=nativePlan({...request,lookupTable,lookupAddresses:addresses});
 const message=Buffer.from(plan.messageBase64,'base64'),tx=VersionedTransaction.deserialize(Buffer.concat([Buffer.from([1]),Buffer.alloc(64),message]));
 tx.message.recentBlockhash=(await rpc('getLatestBlockhash')).value.blockhash;
 const result=await rpc('simulateTransaction',[Buffer.from(tx.serialize()).toString('base64'),{encoding:'base64',sigVerify:false,replaceRecentBlockhash:true,accounts:{encoding:'base64',addresses:[fixture.position,plan.named.session?new PublicKey(plan.named.session).toBase58():fixture.position]}}]);
 assert.equal(result.value.err,null,file+': '+JSON.stringify(result.value.err)+' '+(result.value.logs??[]).slice(-6).join('\n'));
 if(fixture.action==='flash'||fixture.action==='hlp_flash')assert.ok(result.value.accounts[1]===null || (result.value.accounts[1].lamports===0 && result.value.accounts[1].data[0]==='' && result.value.accounts[1].owner==='11111111111111111111111111111111'),'flash session closed');
 rows.push({fixture:directory+'/'+file,sourceCommit:fixture.sourceCommit,action:fixture.action,slot:result.context.slot,computeUnits:result.value.unitsConsumed,status:'passed'});
 console.log(file+' passed');
}
const proof={schemaVersion:1,protocolRevision:lock.revision,validator:'surfpool',kind:'native-plan-rpc-simulation',cases:rows};
writeFileSync(process.env.SURFPOOL_REHEARSAL_PROOF??'/private/tmp/dusk-rollout-surfpool-proof.json',JSON.stringify(proof,null,2)+'\n');
console.log('Surfpool cases: '+rows.length);
