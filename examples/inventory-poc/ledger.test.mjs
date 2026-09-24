import test from 'node:test';
import assert from 'node:assert/strict';
import {openingState,applyEvent,views,canonical} from './ledger.mjs';
import {events} from './scenarios.mjs';
function apply(s,key){return applyEvent(s,events[key]);}
test('split PO keeps transit out of warehouse availability and conserves 3,000',()=>{
  const s=openingState(),before=views(s).summary[0];apply(s,'air');apply(s,'sea');const v=views(s);
  assert.equal(v.summary[0].available,before.available);assert.deepEqual(v.buckets.map(x=>x.units),[500,2500,0]);assert.equal(v.po[0].accounted,3000);
});
test('partial receipts include damage in physical but exclude it from available',()=>{
  const s=openingState();apply(s,'air');apply(s,'airReceipt');const b=s.stock['AMS:BOT-750'];
  assert.equal(b.physical,400);assert.equal(b.damaged,7);assert.equal(views(s).inventory[0].available,383);assert.equal(s.shipments['AIR-500'].received,300);
});
test('duplicate delivery and differently ordered JSON are idempotent',()=>{
  const s=openingState();apply(s,'air');apply(s,'airReceipt');const stock=canonical(s.stock);
  assert.equal(applyEvent(s,Object.fromEntries(Object.entries(events.airReceipt).reverse())).duplicate,true);assert.equal(canonical(s.stock),stock);
});
test('same event key with changed payload is rejected',()=>{
  const s=openingState();apply(s,'air');apply(s,'airReceipt');assert.throws(()=>applyEvent(s,{...events.airReceipt,quantity:1}),/Idempotency conflict/);
});
test('over-dispatch and over-receipt are rejected before quantity changes',()=>{
  const s=openingState();apply(s,'air');const stock=canonical(s.stock);
  assert.throws(()=>applyEvent(s,{...events.sea,quantity:3000}),/supplier balance/);assert.throws(()=>apply(s,'overReceipt'),/Over-receipt/);assert.equal(canonical(s.stock),stock);
});
test('transfer dispatch and partial receipt conserve units including transit',()=>{
  const s=openingState(),before=views(s).summary[0].physical;apply(s,'transfer');assert.equal(views(s).summary[0].physical,before-40);
  apply(s,'transferReceipt');const v=views(s);assert.equal(v.transfers[0].transit,15);assert.equal(v.summary[0].physical+v.transfers[0].transit,before);
  assert.throws(()=>applyEvent(s,{...events.transferReceipt,key:'too-much',quantity:16}),/over-receipt/);
});
test('reservation changes available only; excess reservation is rejected',()=>{
  const s=openingState(),before=views(s).summary[0];apply(s,'reserve');const after=views(s).summary[0];assert.equal(after.physical,before.physical);assert.equal(after.available,before.available-30);
  assert.throws(()=>applyEvent(s,{...events.reserve,key:'excess',quantity:10000}),/Insufficient/);
});
test('sellable and damaged returns have different availability effects',()=>{
  const s=openingState(),before=views(s).summary[0];apply(s,'returnGood');apply(s,'returnBad');const after=views(s).summary[0];assert.equal(after.physical,before.physical+10);assert.equal(after.available,before.available+8);assert.equal(after.damaged,before.damaged+2);
});
test('reject unknown SKU and nonpositive or fractional quantities',()=>{
  for(const quantity of [0,-1,1.5])assert.throws(()=>applyEvent(openingState(),{...events.reserve,quantity}),/whole number/);
  assert.throws(()=>applyEvent(openingState(),{...events.reserve,sku:'UNKNOWN'}),/Unknown/);
});
test('external SKU crosswalk resolves known IDs and quarantines unmapped IDs',()=>{
  const s=openingState();applyEvent(s,{key:'mapped-order',type:'reserve',fc:'AMS',system:'store',externalSku:'SHOP-MUG',quantity:5});
  assert.equal(s.stock['AMS:MUG-350'].reserved,10);
  assert.throws(()=>applyEvent(s,{key:'unmapped',type:'reserve',fc:'AMS',system:'warehouse',externalSku:'UNKNOWN',quantity:5}),/Unmapped/);
  assert.throws(()=>applyEvent(s,{key:'conflict',type:'reserve',fc:'AMS',system:'store',externalSku:'SHOP-MUG',sku:'BOT-750',quantity:5}),/conflicts/);
});
test('freight allocation reconciles shipped and received cost',()=>{
  const s=openingState();for(const k of ['air','sea','airReceipt','seaReceipt'])apply(s,k);const v=views(s);
  assert.deepEqual(v.shipments.map(x=>x.landed_unit),[15,13]);assert.deepEqual(v.shipments.map(x=>x.received_freight),[900,600]);
});
test('reconciliation identifies stale and mismatched data; resolution changes no stock',()=>{
  const s=openingState(),before=canonical(s.stock);apply(s,'drift');apply(s,'reconcile');assert.deepEqual(s.issues.map(x=>x.kind).sort(),['stale','variance']);apply(s,'resolve');assert.equal(s.issues.length,0);assert.equal(canonical(s.stock),before);
});
test('complete scenario yields exact independently calculated balances',()=>{
  const s=openingState();for(const k of ['air','sea','airReceipt','seaReceipt','reserve','transfer','transferReceipt','returnGood','returnBad','airReceipt','retry','drift','reconcile'])apply(s,k);
  const v=views(s);assert.equal(v.summary[0].physical,1375);assert.equal(v.summary[0].available,1282);assert.equal(v.summary[0].reserved,70);assert.equal(v.summary[0].damaged,23);assert.deepEqual(v.buckets.map(x=>x.units),[500,1500,1000]);assert.equal(s.stock['AMS:BOT-750'].physical,468);assert.equal(s.stock['SIN:BOT-750'].physical,707);
});
