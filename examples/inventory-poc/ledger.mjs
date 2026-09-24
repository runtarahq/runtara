// Deterministic mock inventory adapter. All mutation happens on a cloned state;
// the server serializes requests and atomically persists successful transactions.
export const catalog = [
  {sku:'BOT-750', name:'Insulated bottle 750 ml', cost:12, demand:30, supplier:'SUP-B750', store:'SHOP-BOTTLE', warehouse:'WMS-0750'},
  {sku:'MUG-350', name:'Travel mug 350 ml', cost:8, demand:12, supplier:'SUP-M350', store:'SHOP-MUG', warehouse:'WMS-0350'},
  {sku:'TOTE-01', name:'Canvas tote', cost:3, demand:8, supplier:'SUP-T01', store:'SHOP-TOTE', warehouse:'WMS-T01'},
];
export const centers = ['AMS','SIN'];
const now = () => new Date().toISOString();
const date = days => new Date(Date.now()+days*86400000).toISOString().slice(0,10);
export function openingState() {
  const stock = {};
  for (const [i,p] of catalog.entries()) for (const fc of centers) {
    const physical = i===0 ? (fc==='AMS'?100:80) : (i===1?60:40);
    stock[`${fc}:${p.sku}`]={fc,sku:p.sku,physical,reserved:i===0?10:5,damaged:i===0?2:0,opening:physical};
  }
  return {createdAt:now(),stock,po:{id:'PO-3000',sku:'BOT-750',ordered:3000,unit_cost:12},shipments:{},transfers:{},processed:{},attempts:{},audit:[],issues:[],snapshots:{},projection:{},registry:{},revision:0};
}
function need(ok,message){if(!ok)throw Object.assign(new Error(message),{status:409});}
function qty(n){need(Number.isSafeInteger(n)&&n>0,'Quantity must be a positive whole number');return n;}
function balance(s,fc,sku){const b=s.stock[`${fc}:${sku}`];need(b,'Unknown fulfillment center or SKU');return b;}
const available = b => b.physical-b.reserved-b.damaged;
export function canonical(x){return JSON.stringify(x,(_,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.entries(v).sort(([a],[b])=>a.localeCompare(b))):v);}
export function record(s,event,status,message){s.audit.push({id:String(s.audit.length+1),event:event.key||event.type,type:event.type,status,message,at:now()});}
export function applyEvent(s,e){
  need(e && typeof e.key==='string' && e.key.length>0 && e.key.length<160,'A stable event key is required');
  const fingerprint=canonical(e);
  if(s.processed[e.key]){
    need(s.processed[e.key]===fingerprint,'Idempotency conflict: same key, different payload');
    record(s,e,'duplicate','Ignored: identical event already applied');
    return {duplicate:true,event:e.key};
  }
  if(e.externalSku!==undefined){
    need(['supplier','store','warehouse'].includes(e.system),'External SKU system must be supplier, store or warehouse');
    const product=catalog.find(p=>p[e.system]===e.externalSku);
    need(product,'Unmapped external SKU: manual mapping review required');
    need(!e.sku||e.sku===product.sku,'External SKU conflicts with canonical SKU');
    e={...e,sku:product.sku};
  }
  const p=s.po;
  if(e.type==='dispatch'){
    qty(e.quantity);need(['air','sea'].includes(e.mode),'Transport must be air or sea');
    balance(s,e.fc,p.sku);need(!s.shipments[e.shipment],'Shipment reference already exists');
    need(Number.isFinite(e.freight)&&e.freight>=0,'Freight must be nonnegative');
    need(Object.values(s.shipments).reduce((a,x)=>a+x.quantity,0)+e.quantity<=p.ordered,'Dispatch exceeds supplier balance');
    s.shipments[e.shipment]={id:e.shipment,po:p.id,sku:p.sku,fc:e.fc,mode:e.mode,quantity:e.quantity,received:0,damaged:0,freight:e.freight,departure:date(0),eta:date(e.mode==='air'?3:28)};
  }else if(e.type==='receipt'){
    qty(e.quantity);const x=s.shipments[e.shipment];need(x,'Unknown shipment');
    need(!e.sku||e.sku===x.sku,'Receipt SKU does not match shipment');
    const damaged=e.damaged??0;need(Number.isSafeInteger(damaged)&&damaged>=0&&damaged<=e.quantity,'Invalid damaged quantity');
    need(x.received+e.quantity<=x.quantity,'Over-receipt rejected');
    const b=balance(s,x.fc,x.sku);x.received+=e.quantity;x.damaged+=damaged;b.physical+=e.quantity;b.damaged+=damaged;
  }else if(e.type==='reserve'){
    qty(e.quantity);const b=balance(s,e.fc,e.sku);need(available(b)>=e.quantity,'Insufficient available inventory');b.reserved+=e.quantity;
  }else if(e.type==='transfer_dispatch'){
    qty(e.quantity);const b=balance(s,e.from,e.sku);balance(s,e.to,e.sku);
    need(e.from!==e.to,'Transfer requires different centers');need(!s.transfers[e.transfer],'Transfer already exists');need(available(b)>=e.quantity,'Transfer exceeds available inventory');
    b.physical-=e.quantity;s.transfers[e.transfer]={id:e.transfer,sku:e.sku,from:e.from,to:e.to,quantity:e.quantity,received:0};
  }else if(e.type==='transfer_receipt'){
    qty(e.quantity);const x=s.transfers[e.transfer];need(x,'Unknown transfer');need(x.received+e.quantity<=x.quantity,'Transfer over-receipt rejected');
    x.received+=e.quantity;balance(s,x.to,x.sku).physical+=e.quantity;
  }else if(e.type==='return'){
    qty(e.quantity);need(['sellable','damaged'].includes(e.disposition),'Return disposition required');const b=balance(s,e.fc,e.sku);b.physical+=e.quantity;if(e.disposition==='damaged')b.damaged+=e.quantity;
  }else if(e.type==='mock_drift'){
    for(const b of Object.values(s.stock))s.snapshots[`${b.fc}:${b.sku}`]={physical:b.physical,at:now()};
    s.snapshots['SIN:BOT-750']={physical:s.stock['SIN:BOT-750'].physical-7,at:new Date(Date.now()-7200000).toISOString()};
  }else if(e.type==='reconcile'){
    reconcile(s);
  }else if(e.type==='resolve'){
    // Operator confirms the mock WMS snapshot. Never silently change the ledger.
    for(const b of Object.values(s.stock))s.snapshots[`${b.fc}:${b.sku}`]={physical:b.physical,at:now()};
    reconcile(s);
  }else {need(false,`Unknown event type: ${e.type}`);}
  assertInvariants(s);
  s.processed[e.key]=fingerprint;s.revision++;
  record(s,e,'applied',`${e.type} accepted`);
  return {duplicate:false,event:e.key,revision:s.revision};
}
export function reconcile(s){
  s.issues=[];
  for(const b of Object.values(s.stock)){
    const x=s.snapshots[`${b.fc}:${b.sku}`];
    if(!x){s.issues.push({key:`missing:${b.fc}:${b.sku}`,fc:b.fc,sku:b.sku,severity:'warning',kind:'missing_snapshot',message:'No mock WMS snapshot received'});continue;}
    if(Date.now()-Date.parse(x.at)>3600000)s.issues.push({key:`stale:${b.fc}:${b.sku}`,fc:b.fc,sku:b.sku,severity:'warning',kind:'stale',message:'WMS snapshot older than 60 minutes'});
    if(x.physical!==b.physical)s.issues.push({key:`variance:${b.fc}:${b.sku}`,fc:b.fc,sku:b.sku,severity:'critical',kind:'variance',message:`Ledger ${b.physical}; WMS ${x.physical}; variance ${x.physical-b.physical}`});
  }
}
export function assertInvariants(s){
  for(const b of Object.values(s.stock))need([b.physical,b.reserved,b.damaged,available(b)].every(x=>Number.isSafeInteger(x)&&x>=0),'Invalid stock balance');
  for(const x of [...Object.values(s.shipments),...Object.values(s.transfers)])need(x.received>=0&&x.received<=x.quantity,'Invalid received quantity');
  const dispatched=Object.values(s.shipments).reduce((a,x)=>a+x.quantity,0);need(dispatched<=s.po.ordered,'PO over-dispatched');
}
export function views(s){
  const shipments=Object.values(s.shipments).map(x=>({...x,transit:x.quantity-x.received,status:x.received===x.quantity?'received':x.received?'partial':'in_transit',freight_per_unit:x.freight/x.quantity,landed_unit:s.po.unit_cost+x.freight/x.quantity,received_freight:Math.round(x.freight*x.received/x.quantity*100)/100}));
  const inventory=Object.values(s.stock).map(b=>{
    const daily=catalog.find(p=>p.sku===b.sku).demand*(b.fc==='AMS'?0.6:0.4);
    const incoming=shipments.filter(x=>x.fc===b.fc&&x.sku===b.sku).reduce((a,x)=>a+x.transit,0);
    return {...b,available:available(b),incoming,daily,coverage:Math.round(available(b)/daily*10)/10,replenish:Math.max(0,Math.ceil(30*daily-available(b)-incoming)),label:`${b.fc} · ${b.sku}`,as_of:now()};
  });
  const received=shipments.reduce((a,x)=>a+x.received,0),transit=shipments.reduce((a,x)=>a+x.transit,0),supplier=s.po.ordered-received-transit;
  return {inventory,shipments,transfers:Object.values(s.transfers).map(x=>({...x,transit:x.quantity-x.received})),po:[{...s.po,supplier,transit,received,accounted:supplier+transit+received}],buckets:[{bucket:'With supplier',units:supplier},{bucket:'In transit',units:transit},{bucket:'Warehouse receipts',units:received}],issues:s.issues,audit:s.audit.slice(-100),mappings:catalog.map(p=>({...p,key:p.sku})),summary:[{key:'summary',physical:inventory.reduce((a,x)=>a+x.physical,0),available:inventory.reduce((a,x)=>a+x.available,0),reserved:inventory.reduce((a,x)=>a+x.reserved,0),damaged:inventory.reduce((a,x)=>a+x.damaged,0),transit,supplier,received,issues:s.issues.length,revision:s.revision,as_of:now()}]};
}
