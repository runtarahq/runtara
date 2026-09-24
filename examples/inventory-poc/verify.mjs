// Runs ONLY against this dedicated PoC. Reset is explicit to obtain a known baseline.
import assert from 'node:assert/strict';
const base=process.env.INVENTORY_API||'http://127.0.0.1:8090';
const api=process.env.RUNTARA_API||'http://runtara:8080';
async function call(path,body,origin=base){const r=await fetch(origin+path,{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body)});const d=await r.json();if(!r.ok)throw new Error(JSON.stringify(d));return d;}
async function run(key,data={},expected='completed'){
  const state=await call('/state'),workflowId=state.registry.workflows[key].id;
  const start=await call('/api/runtime/workflows/'+workflowId+'/execute',{inputs:{data,variables:{}}},api),id=start.instanceId||start.data?.instanceId;assert.ok(id);
  return waitFor(key,id,expected);
}
async function reportAction(key){
  const state=await call('/state'),reportId=state.registry.reports.inventory.id;
  const rendered=await call(`/api/runtime/reports/${reportId}/render`,{filters:{}},api);
  const response=await fetch(`${api}/api/runtime/reports/${reportId}/blocks/controls/workflow-actions/${key}/execute`,{method:'POST',headers:{'Content-Type':'application/json','Idempotency-Key':crypto.randomUUID()},body:JSON.stringify({trigger:{row:rendered.blocks.controls.data.rows[0]},render:{filters:{}},waitMs:1000})});
  const result=await response.json();assert.ok(response.ok,JSON.stringify(result));assert.ok(result.execution.instanceId);
  return waitFor('native report '+key,result.execution.instanceId,'completed');
}
async function waitFor(key,id,expected){
  for(let i=0;i<180;i++){
    await new Promise(r=>setTimeout(r,500));
    const response=await fetch(api+'/api/runtime/workflows/instances/'+id);
    if(response.status===404)continue;
    const result=await response.json(),instance=result.data||result;
    if(['completed','failed','crashed','cancelled','stopped'].includes(instance.status)){
      assert.equal(instance.status,expected,JSON.stringify(instance));console.log('PASS',key,instance.status,id);return instance;
    }
  }
  throw new Error('Timed out: '+key+' '+id);
}
const quantities=s=>JSON.stringify({inventory:s.inventory.map(({as_of,...r})=>r),po:s.po,shipments:s.shipments,transfers:s.transfers});
await reportAction('reset');
let state=await call('/state');assert.equal(state.po[0].supplier,3000);assert.equal(state.shipments.length,0);
const demo=await reportAction('demo');state=await call('/state');
assert.equal(state.summary[0].physical,1375);assert.equal(state.summary[0].available,1282);assert.equal(state.summary[0].reserved,70);assert.equal(state.summary[0].damaged,23);assert.deepEqual(state.buckets.map(b=>b.units),[500,1500,1000]);assert.equal(state.transfers[0].transit,15);assert.equal(state.issues.length,2);
assert.equal(state.audit.filter(e=>e.event==='receipt-air-002'&&e.status==='failed').length,1);assert.equal(state.audit.filter(e=>e.event==='receipt-air-002'&&e.status==='applied').length,1);
const trace=await call(`/api/runtime/workflows/${state.registry.workflows.demo.id}/instances/${demo.id}/step-events`,undefined,api);
assert.ok(trace.data.events.some(e=>e.payload?.step_id==='event_11_retry'),'Explicit Runtara onError retry step was recorded');
const before=quantities(state);await reportAction('duplicate');assert.equal(quantities(await call('/state')),before);
await run('retry');assert.equal(quantities(await call('/state')),before);
await run('demo');assert.equal(quantities(await call('/state')),before);
await run('reject',{},'failed');assert.equal(quantities(await call('/state')),before);
await run('custom',{event:JSON.stringify({key:'receipt-air-001',type:'receipt',shipment:'AIR-500',quantity:1,damaged:5})},'failed');assert.equal(quantities(await call('/state')),before);
await run('resolve');assert.equal((await call('/state')).issues.length,0);assert.equal(quantities(await call('/state')),before);
// Leave useful alerts visible for the user, using new event identities.
await run('custom',{event:JSON.stringify({key:'drift-verification-final',type:'mock_drift'})});
await run('custom',{event:JSON.stringify({key:'reconcile-verification-final',type:'reconcile'})});
state=await call('/state');assert.equal(state.issues.length,2);
for(const r of Object.values(state.registry.reports)){
  const rendered=await call(`/api/runtime/reports/${r.id}/render`,{filters:{}},api);
  assert.ok(rendered.blocks);for(const [key,block] of Object.entries(rendered.blocks))assert.ok(!block.error&&block.status!=='error',key+': '+JSON.stringify(block));
  console.log('PASS native report rendered:',r.name);
}
console.log('PASS complete E2E: real workflows, partial receipts, conservation, duplicate replay, 503 recovery, rejection, reconciliation, and native reports.');
