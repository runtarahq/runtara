export const events = {
  air:{key:'asn-air-001',type:'dispatch',shipment:'AIR-500',fc:'AMS',mode:'air',quantity:500,freight:1500},
  sea:{key:'asn-sea-001',type:'dispatch',shipment:'SEA-2000',fc:'SIN',mode:'sea',quantity:2000,freight:2000},
  airReceipt:{key:'receipt-air-001',type:'receipt',shipment:'AIR-500',quantity:300,damaged:5},
  seaReceipt:{key:'receipt-sea-001',type:'receipt',shipment:'SEA-2000',quantity:600,damaged:12},
  reserve:{key:'order-1001',type:'reserve',fc:'AMS',sku:'BOT-750',quantity:30},
  transfer:{key:'transfer-out-001',type:'transfer_dispatch',transfer:'TR-001',sku:'BOT-750',from:'AMS',to:'SIN',quantity:40},
  transferReceipt:{key:'transfer-in-001',type:'transfer_receipt',transfer:'TR-001',quantity:25},
  returnGood:{key:'return-001',type:'return',fc:'AMS',sku:'BOT-750',quantity:8,disposition:'sellable'},
  returnBad:{key:'return-002',type:'return',fc:'SIN',sku:'BOT-750',quantity:2,disposition:'damaged'},
  retry:{key:'receipt-air-002',type:'receipt',shipment:'AIR-500',quantity:100,damaged:0,failOnce:true},
  drift:{key:'drift-001',type:'mock_drift'},
  reconcile:{key:'reconcile-001',type:'reconcile'},
  resolve:{key:'resolve-001',type:'resolve'},
  overReceipt:{key:'receipt-invalid-001',type:'receipt',shipment:'AIR-500',quantity:9999,damaged:0},
};
export const scenarios = [
  {key:'split',name:'01 · Split PO: 500 air / 2,000 sea / 500 supplier',description:'Dispatch 2,500 units from PO-3000. No warehouse stock increases. Safe to rerun.',events:['air','sea']},
  {key:'receipts',name:'02 · Partial warehouse receipts + damage quarantine',description:'Receive 300 air (5 damaged) and 600 sea (12 damaged). Requires scenario 01.',events:['airReceipt','seaReceipt']},
  {key:'reservation',name:'03 · Reserve an ecommerce order',description:'Reserve 30 bottles at AMS; physical is unchanged and available decreases.',events:['reserve']},
  {key:'transfer',name:'04 · Transfer 40 units; receive only 25',description:'Dispatch 40 from AMS to SIN; receive 25; 15 remain in transfer transit.',events:['transfer','transferReceipt']},
  {key:'returns',name:'05 · Returns: sellable vs damaged',description:'8 sellable units return to AMS; 2 damaged units to SIN quarantine.',events:['returnGood','returnBad']},
  {key:'duplicate',name:'06 · Replay the same receipt (no double count)',description:'Replays receipt-air-001 exactly. No quantities may change after scenario 02.',events:['airReceipt']},
  {key:'retry',name:'07 · Mock WMS 503 → explicit retry → one receipt',description:'First attempt fails HTTP 503. Runtara onError retries the same key; 100 units applied once.',events:['retry'],retry:true},
  {key:'reconcile',name:'08 · Detect stale WMS data and quantity mismatch',description:'Inject a 2-hour-old SIN snapshot and a 7-unit variance; report both without changing stock.',events:['drift','reconcile']},
  {key:'resolve',name:'09 · Confirm WMS snapshot and resolve alerts',description:'Operator simulation: refresh WMS snapshots from verified ledger and recheck.',events:['resolve']},
  {key:'reject',name:'10 · Reject an over-receipt',description:'Intentionally fails with HTTP 409. Inventory remains unchanged; inspect execution history.',events:['overReceipt']},
  {key:'demo',name:'00 · Run the complete inventory pilot',description:'Runs scenarios 01–08 in sequence, including the retry. Repeat runs are idempotent. Use Reset synthetic data in the control-tower report to start again.',events:['air','sea','airReceipt','seaReceipt','reserve','transfer','transferReceipt','returnGood','returnBad','airReceipt','retry','drift','reconcile'],retry:true},
];
export const immediate=value=>({valueType:'immediate',value});
export const reference=value=>({valueType:'reference',value});
function httpStep(id,name,endpoint,body){return {id,name,stepType:'Agent',agentId:'http',capabilityId:'http-request',inputMapping:{method:immediate('POST'),url:immediate('http://inventory:8090'+endpoint),body:body.valueType==='immediate'?immediate(JSON.stringify(body.value)):body,body_type:immediate('json'),response_type:immediate('json'),fail_on_error:immediate(true)}};}
export function graphFor(scenario){
  const steps={},executionPlan=[];
  const ids=scenario.events.map((_,i)=>`event_${i+1}`);
  for(const [i,event] of scenario.events.entries()){
    const id=ids[i],next=ids[i+1]||'publish';
    steps[id]=httpStep(id,event==='retry'?'WMS receipt — fail once, then retry':`${i+1}. ${events[event].type} · ${events[event].key}`,'/events',immediate(events[event]));
    executionPlan.push({fromStep:id,toStep:next});
    if(event==='retry'){
      // Make the error edge visible: this example retries explicitly rather
      // than letting the agent's default automatic retries consume the 503.
      steps[id].maxRetries=0;
      steps[id+'_retry']=httpStep(id+'_retry','Retry same idempotency key after mock 503','/events',immediate(events[event]));
      executionPlan.push({fromStep:id,toStep:id+'_retry',label:'onError'},{fromStep:id+'_retry',toStep:next});
    }
  }
  steps.publish=httpStep('publish','Refresh native Runtara report data','/publish',immediate({}));
  steps.done={id:'done',name:'Report projection refreshed',stepType:'Finish',inputMapping:{publication:reference('steps.publish.outputs.body')}};
  executionPlan.push({fromStep:'publish',toStep:'done'});
  return {name:scenario.name,description:scenario.description,inputSchema:{},entryPoint:ids[0],steps,executionPlan};
}
export function customGraph(){
  const graph=graphFor({...scenarios[0],events:['air']});
  graph.name='11 · Process a custom inventory event';graph.description='Input field event is a JSON string, e.g. {"key":"your-unique-id","type":"receipt","shipment":"AIR-500","quantity":25,"damaged":0}. Reuse the same key only for the identical payload.';
  graph.inputSchema={event:{type:'string',required:true}};
  graph.steps.event_1.name='Validate and apply inventory event';graph.steps.event_1.inputMapping.body=reference('data.event');return graph;
}
export function resetGraph(){
  const graph=graphFor({...scenarios[0],events:['air']});
  graph.name='00 · Reset synthetic pilot data';
  graph.description='Restore only this PoC’s synthetic opening balances, PO and mock event ledger. Keeps workflows, reports and Runtara execution history. Run the complete pilot again afterwards.';
  graph.steps.event_1=httpStep('event_1','Reset only synthetic inventory data','/reset',immediate({}));
  return graph;
}
