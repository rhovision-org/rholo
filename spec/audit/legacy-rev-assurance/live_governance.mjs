// #412 Phase IV-A. A narrow real-chain witness, not a production claims contract.
// Only public disposable fixture keys on an isolated GitHub runner.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const QOS = resolve(process.env.QOS412_QOS ?? "");
const GOV = resolve(process.env.QOS412_GOV ?? "");
const NODE = "http://127.0.0.1:40403";
const q = JSON.stringify;
const sha = x => createHash("sha256").update(x).digest("hex");
const sleep = ms => new Promise(r=>setTimeout(r,ms));
const report = {schema:"legacy-rev-live/1", scope:"disposable single-validator Rholo; synthetic fixed unanimous roll; not production policy", stages:[]};
function stage(name, detail={}) {report.stages.push({name,verdict:"PASS",...detail});console.log("REV412 PASS",name);}
function save() {
  const raw=JSON.stringify(report,null,2)+"\n";
  writeFileSync("rev412-live.json",raw);
  writeFileSync("rev412-live.SHA256SUMS",sha(raw)+"  rev412-live.json\n");
}
function canon(x) {
  if(Array.isArray(x)) return "["+x.map(canon).join(",")+"]";
  if(x!==null&&typeof x==="object")return "{"+Object.keys(x).sort().map(k=>q(k)+":"+canon(x[k])).join(",")+"}";
  return q(x);
}
function decode(n) {
  if(n===null||typeof n!=="object")return n;
  if(Array.isArray(n))return n.map(decode);
  const [tag,...rest]=Object.keys(n);
  if(rest.length||!tag?.startsWith("Expr"))return n;
  let b=n[tag];
  if(b&&typeof b==="object"&&!Array.isArray(b)&&Object.keys(b).length===1&&"data" in b)b=b.data;
  if(tag==="ExprMap")return Object.fromEntries((Array.isArray(b)?b:Object.entries(b??{})).map(([k,v])=>[String(decode(k)),decode(v)]));
  if(["ExprList","ExprTuple","ExprSet","ExprPar"].includes(tag))return b.map(decode);
  return b;
}
async function api(path, body) {
  const res=await fetch(NODE+path,{...(body===undefined?{}:{method:"POST",headers:{"content-type":"application/json"},body:q(body)}),signal:AbortSignal.timeout(10000)});
  assert.ok(res.ok, path+" HTTP "+res.status);
  return res.json();
}
async function finalized() {
  const r=await api("/api/last-finalized-block"), b=r.blockInfo??r;
  assert.match(b.blockHash,/^[a-f0-9]{64}$/i);
  assert.ok(Number.isSafeInteger(b.blockNumber));
  return {hash:b.blockHash,height:b.blockNumber};
}
async function at(term,b) {
  const r=await api("/api/explore-deploy-by-block-hash",{term,blockHash:b.hash,usePreStateHash:false});
  assert.equal(r.block?.blockHash,b.hash,"exact block pin required");
  assert.ok(Array.isArray(r.expr),"missing expr");
  return r.expr.map(decode);
}
async function finalRead(term,check,label) {
  for(let i=0;i<40;i++){
    const b=await finalized();
    const v=await at(term,b);
    if(check(v))return {block:b,values:v};
    report.lastUnmatchedFinalizedRead={label,block:b,values:v};
    await sleep(1500);
  }
  throw Error("finalized read timeout: "+label);
}
const uri = s => {assert.match(s,/^rho:id:[a-z0-9]+$/);return s;};
const response = (v,tag) => v.find(x=>Array.isArray(x)&&x[0]===tag);
const wrap = s => `new deployId(\`rho:rchain:deployId\`), ans in { ${s} | for (@r <- ans) { return!(r) | deployId!(r) } }`;
const lookup = (id,facet,call) => `new lookup(\`rho:registry:lookup\`), stored, capsCh in {
 lookup!(\`${uri(id)}\`, *stored) | for (@record <- stored) {
  match record { (_,c) => { capsCh!(c) } c => { capsCh!(c) } } |
  for (@caps <- capsCh) { match caps {
   {${q(facet)}: found, ..._} => { ${call} }
   _ => { ans!(( "refused", "missing facet" )) }
  } }
 }
}`;
const readGate=(id)=>`new return, lookup(\`rho:registry:lookup\`), stored, capsCh in {
 lookup!(\`${uri(id)}\`, *stored) | for (@record <- stored) {
  match record { (_,c) => { capsCh!(c) } c => { capsCh!(c) } } |
  for (@caps <- capsCh) { match caps {
   {"read": found, ..._} => { @found!(*return) }
   _ => { return!("missing gate") }
  } }
 }
}`;
const balance=(address)=>`new return, rv(\`rho:rchain:revVault\`), ch in {
 rv!("findOrCreate", ${q(address)}, *ch) | for (@r <- ch) {
  match r { (true, handle) => { @handle!("balance", *return) } _ => { return!("bad vault") } }
 }
}`;
// A one-claim contract. No external input can replace amount, owner, issue, roll
// or manifest. It rechecks live Issue/4 facts, not opener-written results.
// Treasury auth is a private unforgeable; re-resolve the handle at spend time.
function installGate(issue, p, title, iid) {
  const voters=`{${p.roll.map(a=>q(a)+": true").join(", ")}}`;
  const ballots=`{${p.roll.map(a=>q(a)+': ["approve"]').join(", ")}}`;
  const predicate=[
    'i.getOrElse("status", "") == "closed"',
    'i.getOrElse("mode", "") == "approval"',
    'i.getOrElse("title", "") == '+q(title),
    'i.getOrElse("gid", "") == '+q(p.group),
    'i.getOrElse("by", "") == '+q(p.opener),
    'i.getOrElse("options", []) == ["approve", "reject"]',
    'i.getOrElse("voters", {}) == '+voters,
    'i.getOrElse("guests", {}) == {}',
    'i.getOrElse("ballots", {}) == '+ballots,
    'i.getOrElse("rollLog", []) == []'
  ].map(x=>"("+x+")").join(" and ");
  return `new deployId(\`rho:rchain:deployId\`), insert(\`rho:registry:insertArbitrary\`),
 lookup(\`rho:registry:lookup\`), rv(\`rho:rchain:revVault\`), ra(\`rho:rev:address\`),
 unf, addrCh, authCh, state, claim, read, registered in {
 ra!("fromUnforgeable", *unf, *addrCh) |
 rv!("unforgeableAuthKey", *unf, *authCh) |
 for (@address <- addrCh & @auth <- authCh) {
  state!(false) |
  contract read(ret) = { for (@paid <- state) { state!(paid) | ret!((paid,address)) } } |
  contract claim(ret) = {
   for (@paid <- state) {
    if (paid) { state!(paid) | ret!(("refused","consumed")) }
    else {
     new stored, capsCh, issueCh in {
      lookup!(\`${uri(issue)}\`, *stored) |
      for (@record <- stored) {
       match record { (_,c) => { capsCh!(c) } c => { capsCh!(c) } } |
       for (@caps <- capsCh) {
        match caps {
         {"read": found, ..._} => {
          @found!("issueOf", [${q(iid)}], *issueCh) |
          for (@i <- issueCh) {
           if (${predicate}) {
            new vaultCh, payCh in {
             rv!("findOrCreate", address, *vaultCh) |
             for (@created <- vaultCh) {
              match created {
               (true, handle) => {
                @handle!("transfer", ${q(p.owner)}, ${p.amount}, auth, *payCh) |
                for (@res <- payCh) {
                 match res {
                  (true, _) => { state!(true) | ret!(("paid", ${p.amount})) }
                  (false, reason) => { state!(false) | ret!(("refused","treasury transfer")) }
                  _ => { state!(false) | ret!(("refused","unexpected transfer reply")) }
                 }
                }
               }
               _ => { state!(false) | ret!(("refused","vault unavailable")) }
              }
             }
            }
           } else { state!(false) | ret!(("refused","governance")) }
          }
         }
         _ => { state!(false) | ret!(("refused","missing issue facet")) }
        }
       }
      }
     }
    }
   }
  } |
  insert!({"read": bundle+{*read}, "claim": bundle+{*claim}}, *registered) |
  for (@id <- registered) { return!(("installed-gate",id,address)) | deployId!(("installed-gate",id,address)) }
 }
}`;
}
function approved(i,p,title) {
  if(!i||typeof i!=="object"||Array.isArray(i))return false;
  const roll=Object.fromEntries(p.roll.map(a=>[a,true]));
  const ballots=Object.fromEntries(p.roll.map(a=>[a,["approve"]]));
  return i.status==="closed"&&i.mode==="approval"&&i.title===title&&i.gid===p.group&&i.by===p.opener&&
    canon(i.options)===canon(["approve","reject"])&&canon(i.voters)===canon(roll)&&
    canon(i.guests)===canon({})&&canon(i.ballots)===canon(ballots)&&canon(i.rollLog??[])==="[]";
}
async function main(){
  assert.equal(process.env.GITHUB_ACTIONS,"true");
  assert.ok(process.env.QOS412_DATA?.startsWith("/home/runner/")||process.env.QOS412_DATA?.startsWith("/tmp/"));
  const C=await import(pathToFileURL(GOV+"/src/chain/rgov-core.js"));
  const {DEFAULT_CONFIG,deployTerm,nodeStatus,revAddressOf}=await import(pathToFileURL(QOS+"/scripts/qos-cli/rholang-client.mjs"));
  const readKey=n=>readFileSync(QOS+"/scripts/localnet/pk.txt","utf8").split("\n").find(l=>l.startsWith(n+"="))?.split("=")[1].trim();
  const keys=Object.fromEntries(["deployer","alice","bob","carol"].map(n=>[n,readKey(n)]));
  for(const k of Object.values(keys))assert.match(k,/^[a-f0-9]{64}$/i);
  const addr=Object.fromEntries(Object.entries(keys).map(([n,k])=>[n,revAddressOf(k)]));
  assert.equal(new Set(Object.values(addr)).size,4);
  const st=await nodeStatus({url:NODE});
  assert.ok(st.networkId&&st.shardId);
  report.node={network:st.networkId,shard:st.shardId,version:process.env.QOS412_VERSION,sha256:process.env.QOS412_BINARY_SHA};
  report.sources={rholoResearch:process.env.GITHUB_SHA,rhogov:process.env.QOS412_GOV_SHA,quantumOs:process.env.QOS412_QOS_SHA};
  async function write(who,term,tag,label) {
    const r=await deployTerm({...DEFAULT_CONFIG,url:NODE,key:keys[who],shard:st.shardId,phloLimit:8000000},term,{waitAttempts:0});
    assert.ok(r.ok&&r.sig,label+" submission refused: "+String(r.message??"missing reply").slice(0,400));
    for(let i=0;i<40;i++){
      let j;
      try { j=await api("/api/v1/deploy-status/"+encodeURIComponent(r.sig)); } catch(e) { if(!/HTTP 404|HTTP 503/.test(String(e)))throw e;await sleep(1500);continue; }
      assert.ok(!j.ProcessedWithError,label+" processed with error: "+JSON.stringify(j.ProcessedWithError??null).slice(0,400));
      if(j.ProcessedWithSuccess){
        const v=(j.ProcessedWithSuccess.deployResult??[]).map(decode);
        const value=response(v,tag);
        assert.ok(value,label+" missing expected "+tag+" reply: "+JSON.stringify(v).slice(0,180));
        report.stages.push({name:label,verdict:"PROCESSED",deployId:r.sig,result:value});
        console.log("REV412 PROCESSED",label,JSON.stringify(value).slice(0,180));
        return value;
      }
      await sleep(1500);
    }
    throw Error(label+" no processed outcome");
  }
  const installed=await write("deployer",C.installIssueProgram(),"installed","install actual RhoGov Issue/4");
  const issue=uri(installed[2]);
  // Fixed research claim; manifest/policy content hashes are test-domain values.
  const p={schema:"legacy-rev-claim/1",network:st.networkId,shard:st.shardId,
    manifest:sha("synthetic manifest 412"),policy:sha("fixed two-of-two fixture"),
    source:"synthetic-source-412",owner:addr.carol,amount:70,group:"legacy-rev-412",
    opener:addr.deployer,roll:[addr.alice,addr.bob].sort(),issueContract:issue};
  const claimHash=sha(canon(p));
  const title="legacy-rev-claim/1:"+encodeURIComponent(canon(p)), iid="rev412-"+claimHash;
  assert.deepEqual(JSON.parse(decodeURIComponent(title.slice("legacy-rev-claim/1:".length))),p);
  report.claim=p;report.issue={uri:issue,id:iid,titleHash:sha(title),claimHash};
  await write("deployer",C.openIssueProgram(issue,iid,p.group,title,"approval",["approve","reject"],p.roll),"opened","open exact claim issue");
  const gateResult=await write("deployer",installGate(issue,p,title,iid),"installed-gate","install one-claim private treasury gate");
  const gate=uri(gateResult[1]), treasury=gateResult[2];
  report.gate={uri:gate,treasury};
  assert.match(treasury,/^[1-9A-HJ-NP-Za-km-z]{20,60}$/);
  const claimTerm=wrap(lookup(gate,"claim","@found!(*ans)"));
  const fund=wrap(`new rv(\`rho:rchain:revVault\`), deployerId(\`rho:rchain:deployerId\`), ch in {
    rv!("transfer", *deployerId, ${q(treasury)}, 100, *ch) |
    for (@r <- ch) { match r { Nil => { ans!(("fund", true)) } _ => { ans!(("fund", false)) } } }
  }`);
  const funding=await write("deployer",fund,"fund","fund fresh private treasury");
  assert.deepEqual(funding,["fund",true],"classic native transfer must return Nil; normalized by Rholang");
  const initial=await finalRead(balance(treasury),v=>v.length===1&&v[0]===100,"treasury funding");
  stage("finalized treasury funding",{block:initial.block,amount:100});
  const base=await at(balance(p.owner),initial.block);
  assert.ok(Number.isSafeInteger(base[0]));
  report.initial={block:initial.block,treasury:100,beneficiary:base[0]};
  // Opener's proposed result is explicitly NOT a ballot authorization.
  await write("deployer",C.writeProgram(issue,"self","propose",[q(iid),q("approve")]),"proposed","opener writes claimed approval before ballots");
  const denied=await write("deployer",claimTerm,"refused","claim before vote close");
  assert.equal(denied[1],"governance");
  await write("alice",C.castProgram(issue,iid,["approve"]),"cast","alice signed ballot");
  await write("bob",C.castProgram(issue,iid,["approve"]),"cast","bob signed ballot");
  const closed=await write("deployer",C.closeIssueProgram(issue,iid),"closed","close issue");
  assert.equal(closed[1],iid);
  const readIssue=C.readProgram(issue,"issueOf",[q(iid)]);
  const voted=await finalRead(readIssue,v=>v.length===1&&approved(v[0],p,title),"exact finalized unanimous issue");
  assert.deepEqual(await at(balance(treasury),voted.block),[100]);
  assert.deepEqual(await at(balance(p.owner),voted.block),base);
  assert.deepEqual(await at(readGate(gate),voted.block),[[false,treasury]]);
  report.decisionReceipt={block:voted.block,issue:report.issue,record:voted.values[0]};
  report.decisionReceipt.sha256=sha(canon(report.decisionReceipt));
  stage("finalized exact ballot receipt; pre-approval refusal conserved funds",{block:voted.block});
  const paid=await write("deployer",claimTerm,"paid","execute real vault settlement");
  assert.equal(paid[1],70);
  const settled=await finalRead(readGate(gate),v=>canon(v)===canon([[true,treasury]]),"finalized consumed state");
  assert.deepEqual(await at(balance(treasury),settled.block),[30]);
  assert.deepEqual(await at(balance(p.owner),settled.block),[base[0]+70]);
  stage("finalized reserve debit + beneficiary credit + consumed flag",{block:settled.block,reserve:30,beneficiaryDelta:70});
  const replay=await write("bob",claimTerm,"refused","replay from another deployer");
  assert.equal(replay[1],"consumed");
  // Require a later finalized block for replay readback, not the pre-replay state.
  let replayBlock;
  for(let i=0;i<40;i++){const b=await finalized();if(b.height>settled.block.height){replayBlock=b;break;}await sleep(1500);}
  assert.ok(replayBlock,"no later finalized block");
  assert.deepEqual(await at(readGate(gate),replayBlock),[[true,treasury]]);
  assert.deepEqual(await at(balance(treasury),replayBlock),[30]);
  assert.deepEqual(await at(balance(p.owner),replayBlock),[base[0]+70]);
  stage("cross-deployer replay refused; finalized balances unchanged",{block:replayBlock});
  report.result="PASS_NARROW_DISPOSABLE_VAULT_WITNESS";
}
try{await main();}catch(e){report.result="FAIL_OR_BLOCKED";report.failure=String(e.stack??e).slice(0,1800);console.error("REV412 FAIL",report.failure);process.exitCode=1;}finally{save();}
