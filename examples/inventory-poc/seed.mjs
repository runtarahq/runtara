import {scenarios,graphFor,customGraph,resetGraph} from './scenarios.mjs';
const api=process.env.RUNTARA_API||'http://runtara:8080';
const adapter=process.env.INVENTORY_API||'http://127.0.0.1:8090';
async function call(method,path,body,base=api+'/api/runtime'){
  const r=await fetch(base+path,{method,headers:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(300000)});
  const data=await r.json();if(!r.ok||data.success===false)throw new Error(`${method} ${path} (${r.status}): ${JSON.stringify(data)}`);return data;
}
const state=await call('GET','/state',undefined,adapter);
const registry=state.registry||{};registry.workflows||={};registry.schemas||={};registry.reports||={};
const save=()=>call('POST','/registry',registry,adapter);
const existing=await call('GET','/workflows?page=0&size=500');
const names=new Map((existing.data?.content||[]).map(w=>[w.name,w]));
for(const scenario of [...scenarios,{key:'custom',name:'11 · Process a custom inventory event'},{key:'reset',name:'00 · Reset synthetic pilot data'}]){
  if(registry.workflows[scenario.key]&&!process.env.UPDATE_WORKFLOWS?.split(',').includes(scenario.key)){
    const existing=registry.workflows[scenario.key];
    if(!existing.tracked){
      await call('PUT',`/workflows/${existing.id}/versions/${existing.version}/track-events`,{trackEvents:true});
      await call('POST',`/workflows/${existing.id}/versions/${existing.version}/compile`);
      existing.tracked=true;await save();
    }
    continue;
  }
  const graph=scenario.key==='custom'?customGraph():scenario.key==='reset'?resetGraph():graphFor(scenario);
  console.log('Seeding workflow:',graph.name);
  const created=registry.workflows[scenario.key]||names.get(graph.name)||await call('POST','/workflows/create',{name:graph.name,description:graph.description});
  const id=created.data?.id||created.id;
  const updated=await call('POST',`/workflows/${id}/update`,{executionGraph:graph,trackEvents:true});
  const version=updated.version||updated.data?.version;
  await call('POST',`/workflows/${id}/versions/${version}/compile`);
  await call('POST',`/workflows/${id}/versions/${version}/set-current`);
  registry.workflows[scenario.key]={id,name:graph.name,version,description:graph.description,tracked:true};await save();
}

const schemaSpecs={
  PilotInventory:['inventory','fc sku label as_of','physical reserved damaged opening available incoming replenish','daily coverage'],
  PilotShipment:['shipments','reference po sku fc mode status departure eta','quantity received damaged transit','freight freight_per_unit landed_unit received_freight'],
  PilotTransfer:['transfers','reference sku source_fc destination_fc','quantity received transit',''],
  PilotPO:['po','reference sku','ordered supplier transit received accounted','unit_cost'],
  PilotBucket:['buckets','bucket','units',''],
  PilotIssue:['issues','key fc sku severity kind message','',''],
  PilotAudit:['audit','reference event type status message at','',''],
  PilotMapping:['mappings','key sku name supplier store warehouse','','cost demand'],
  PilotSummary:['summary','key as_of','physical available reserved damaged transit supplier received issues revision',''],
  PilotControl:['controls','key label','',''],
};
for(const [name,[view,strings,integers,decimals]] of Object.entries(schemaSpecs)){
  if(registry.schemas[name])continue;
  const columns=[];
  for(const [keys,type] of [[strings,'string'],[integers,'integer'],[decimals,'decimal']])for(const key of keys.split(' ').filter(Boolean))columns.push({name:key,type});
  let created;
  try{created=await call('GET',`/object-model/schemas/name/${name}`);}catch{created=await call('POST','/object-model/schemas',{name,tableName:name.replace(/[A-Z]/g,(c,i)=>(i?'_':'')+c.toLowerCase()),columns});}
  const id=created.schemaId||created.schema?.id;
  if(!id)throw new Error('Missing schema id: '+JSON.stringify(created));
  registry.schemas[name]={id,view,fields:columns.map(c=>c.name)};await save();
}
await call('POST','/publish',{},adapter);

const source=schema=>({schema,mode:'filter'});
const md=(id,content)=>({id,type:'markdown',markdown:{content}});
const table=(id,title,schema,fields)=>({id,type:'table',title,source:source(schema),table:{columns:fields.map(f=>typeof f==='string'?{field:f,label:f.replace(/_/g,' ')}:f)}});
const chart=(id,title,schema,x,fields,kind='bar')=>({id,type:'chart',title,source:{schema,mode:'aggregate',groupBy:[x],aggregates:fields.map(field=>({field,alias:field,op:'sum'}))},chart:{kind,x,series:fields.map(field=>({field,label:field}))}});
const metric=(field,label)=>({id:'metric_'+field,type:'metric',title:label,source:{schema:'PilotSummary',mode:'aggregate',aggregates:[{field,alias:field,op:'sum'}]},metric:{valueField:field,label,format:'number'}});
const blockNode=id=>({id:'node_'+id,type:'block',blockId:id});
function definition(blocks,metrics=[]){return {definitionVersion:1,filters:[],blocks,layout:{id:'root',columns:1,items:[...blocks.filter(b=>!metrics.includes(b.id)).map(b=>({id:'item_'+b.id,child:blockNode(b.id)}))]}};}
function dashboard(blocks){
  const d=definition(blocks);const metrics=blocks.filter(b=>b.type==='metric');
  d.layout.items=d.layout.items.filter(x=>!metrics.some(m=>x.child.blockId===m.id));
  if(metrics.length)d.layout.items.splice(1,0,{id:'metrics',child:{id:'metrics_grid',type:'grid',columns:4,items:metrics.map(m=>({id:'item_'+m.id,child:blockNode(m.id)}))}});
  return d;
}
function launcher(keys){return table('controls','Run a workflow · refresh the page after completion','PilotControl',[
  {field:'label',label:'Pilot controls'},...keys.map(key=>({field:key,label:key,type:'workflow_button',workflowAction:{id:key,workflowId:registry.workflows[key].id,label:({demo:'Run full pilot',reset:'Reset synthetic data',duplicate:'Replay receipt',retry:'Retry WMS receipt',reconcile:'Inject & detect issues',resolve:'Confirm WMS snapshot',reject:'Test over-receipt',split:'Dispatch air + sea',receipts:'Receive partial shipments',transfer:'Transfer between centers',returns:'Process returns',reservation:'Reserve order'})[key]||key,runningLabel:'Running…',successMessage:'Workflow completed; refresh report to see all updated blocks',reloadBlock:true,context:{mode:'row'}}}))
]);}
const intro='**LOCAL PROOF OF CONCEPT · SYNTHETIC DATA · MOCKED EXTERNAL SYSTEMS**\n\nTwo fulfillment centers (Amsterdam / Singapore), three representative SKUs. Available = physical − reserved − damaged. Supplier and in-transit quantities are never available stock. Values refresh when workflows publish; reload the report after a run. This is not a validated production inventory platform or a volume benchmark.';
const reportSpecs={
  inventory:{name:'Inventory pilot · Control tower',slug:'inventory-pilot-control-tower',definition:dashboard([
    md('intro','# Inventory control tower\n\n'+intro+'\n\n**Start here:** Run the full pilot below, then click Refresh. To replay from opening balances, use Reset synthetic data first. Each button runs a real Runtara workflow. Repeated events are deduplicated.\n\n[All pilot reports](http://localhost:3080/ui/reports) · [Workflow catalog and execution history](http://localhost:3080/ui/workflows) · [Custom event workflow](http://localhost:3080/ui/workflows/'+registry.workflows.custom.id+')'),
    metric('physical','Physical at warehouses'),metric('available','Available to promise'),metric('reserved','Reserved'),metric('damaged','Damaged / quarantined'),
    launcher(['demo','reset','duplicate','retry']),
    chart('availability','Available, reserved and damaged by SKU / center','PilotInventory','label',['available','reserved','damaged']),
    table('inventory','Balances and coverage (30-day planning horizon)','PilotInventory',['fc','sku','physical','reserved','damaged','available','incoming','daily','coverage','replenish','as_of']),
    md('planning','Coverage is available / synthetic daily demand. Replenishment is max(0, 30 × daily demand − available − inbound). It is a planning estimate; incoming stock is shown separately and does not prevent a stockout before its ETA.'),
    table('mappings','Canonical SKU crosswalk · synthetic verified opening data','PilotMapping',['sku','name','supplier','store','warehouse','cost']),
  ])},
  inbound:{name:'Inventory pilot · Purchasing and freight',slug:'inventory-pilot-purchasing',definition:dashboard([
    md('intro','# Purchasing, inbound freight and landed cost\n\n'+intro),
    metric('supplier','Still with supplier'),metric('transit','Inbound transit'),metric('received','Confirmed PO receipts'),
    launcher(['split','receipts','transfer','returns']),
    chart('po_buckets','PO-3000 · mutually exclusive quantity buckets','PilotBucket','bucket',['units'],'donut'),
    table('po','Conservation check: supplier + transit + received = 3,000','PilotPO',['reference','sku','ordered','supplier','transit','received','accounted']),
    table('shipments','Air and sea · independent ETAs and partial receipts','PilotShipment',['reference','fc','mode','status','quantity','received','damaged','transit','departure','eta','freight','freight_per_unit','landed_unit','received_freight']),
    chart('cost','Landed unit cost by shipment (USD)','PilotShipment','reference',['landed_unit']),
    table('transfers','Inter-center transfers · transit is excluded from both warehouses','PilotTransfer',['reference','sku','source_fc','destination_fc','quantity','received','transit']),
    md('cost_note','Freight allocated by shipped units: AIR $1,500 / 500 = $3 per unit; SEA $2,000 / 2,000 = $1 per unit. Base cost $12 gives landed units of $15 / $13. Received freight = freight × received / shipped, including damaged receipts. Duties, FX, insurance, actual carrier invoices, and weight/value allocation are outside this PoC.'),
  ])},
  reliability:{name:'Inventory pilot · Integration health and audit',slug:'inventory-pilot-integration-health',definition:dashboard([
    md('intro','# Reliability and reconciliation\n\n'+intro+'\n\nScenario 07 generates a real HTTP 503 inside the mock adapter; the Runtara onError edge retries the same event. Scenario 10 intentionally fails. Scenario 08 creates stale data and a quantity mismatch; scenario 09 resolves them only after simulated operator confirmation.'),
    metric('issues','Open reconciliation alerts'),launcher(['reconcile','resolve','reject']),
    table('issues','Unresolved stale data and quantity discrepancies','PilotIssue',['fc','sku','severity','kind','message']),
    {...table('audit','Adapter audit · applied / duplicate / rejected / failed','PilotAudit',['at','event','type','status','message']),source:{...source('PilotAudit'),orderBy:[{field:'at',direction:'desc'}],limit:100}},
    {id:'runs',type:'table',title:'Complete-pilot Runtara execution history',source:{kind:'workflow_runtime',entity:'instances',workflowId:registry.workflows.demo.id,mode:'filter',limit:30},table:{columns:[{field:'id',label:'Instance'},{field:'status',label:'Status'},{field:'createdAt',label:'Started'},{field:'durationSeconds',label:'Duration (s)'}]}},
    md('support','**Recovery:** inspect the failed run and adapter audit, correct a bad SKU/quantity, then submit a new event key. For transport failures, retry the exact same key and payload. A changed payload with the same key is rejected. Report publication can be retried without replaying stock changes.\n\n**PoC limits:** single adapter writer and local JSON persistence; no production HA, distributed event ordering, automatic retry backoff, periodic WMS polling or 60k-order/month load certification. All supplier/store/WMS connections are mocked.'),
  ])},
};
for(const [key,report] of Object.entries(reportSpecs)){
  const old=registry.reports[key];
  const response=await call(old?'PUT':'POST',old?`/reports/${old.id}`:'/reports',{...report,status:'published',tags:['inventory-poc']});
  registry.reports[key]={id:response.report.id,name:report.name};await save();
  const render=await call('POST',`/reports/${response.report.id}/render`,{filters:{}});
  const errors=Object.entries(render.blocks||{}).filter(([,b])=>b.error||b.status==='error');
  if(errors.length)throw new Error('Report render errors: '+JSON.stringify(errors));
  console.log('Report:',report.name,response.report.id);
}
console.log('Seed complete. Open http://localhost:3080/ui/reports');
