import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import {openingState,applyEvent,record,views,reconcile} from './ledger.mjs';

const file=process.env.STATE_FILE||'/tmp/runtara-inventory-poc-state.json';
fs.mkdirSync(path.dirname(file),{recursive:true});
let state=fs.existsSync(file)?JSON.parse(fs.readFileSync(file,'utf8')):openingState();
function save(){const fd=fs.openSync(file+'.tmp','w');try{fs.writeFileSync(fd,JSON.stringify(state));fs.fsyncSync(fd);}finally{fs.closeSync(fd);}fs.renameSync(file+'.tmp',file);}
save();
const api=process.env.RUNTARA_API||'http://127.0.0.1:3080';
async function request(method,url,body){const r=await fetch(api+'/api/runtime'+url,{method,headers:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(300000)});const data=await r.json();if(!r.ok||data.success===false)throw new Error(`${method} ${url}: ${JSON.stringify(data)}`);return data;}

// Reports read native Object Model rows. Fixed natural keys + remembered IDs
// make retries updates, not duplicate rows. Publication errors remain visible.
async function publish(){
  const data=views(state);
  data.controls=[{key:'control',label:'Run scenario workflows, then refresh this report'}];
  for(const [name,spec] of Object.entries(state.registry.schemas||{})){
    const rows=data[spec.view]||[];const active=new Set();
    for(const row of rows){
      const key=String(row.key||row.label||row.id||row.bucket||row.sku);
      active.add(key);const ref=`${name}:${key}`,id=state.projection[ref];
      const properties=Object.fromEntries(spec.fields.map(k=>[k,(k==='reference'?row.id:k==='source_fc'?row.from:k==='destination_fc'?row.to:row[k])??'']));
      if(id)await request('PUT',`/object-model/instances/${spec.id}/${id}`,{properties});
      else {const created=await request('POST','/object-model/instances',{schemaName:name,properties});state.projection[ref]=created.instanceId;save();}
    }
    for(const [ref,id] of Object.entries(state.projection))if(ref.startsWith(name+':')&&!active.has(ref.slice(name.length+1))){await request('DELETE',`/object-model/instances/${spec.id}/${id}`);delete state.projection[ref];save();}
  }
  state.publishedAt=new Date().toISOString();delete state.publishError;save();return {publishedAt:state.publishedAt,revision:state.revision};
}
let queue=Promise.resolve();
const server=http.createServer(async(req,res)=>{
  let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>1000000){res.writeHead(413);res.end();return;}}
  const run=async()=>{
    const url=new URL(req.url,'http://local');let body={};try{body=raw?JSON.parse(raw):{};}catch{throw Object.assign(new Error('Invalid JSON'),{status:400});}
    if(req.method==='GET'&&url.pathname==='/health')return {ok:true};
    if(req.method==='GET'&&url.pathname==='/state')return {...views(state),registry:state.registry,publishedAt:state.publishedAt,publishError:state.publishError};
    if(req.method==='POST'&&url.pathname==='/registry'){state.registry=body;save();return {ok:true};}
    if(req.method==='POST'&&url.pathname==='/publish')return publish();
    if(req.method==='POST'&&url.pathname==='/reset'){
      const old=state;state=openingState();state.registry=old.registry;state.projection=old.projection;
      record(state,{key:'reset',type:'reset'},'applied','Synthetic opening balances restored by user');save();return publish();
    }
    if(req.method==='POST'&&url.pathname==='/events'){
      if(body.failOnce&&!state.processed[body.key]&&!state.attempts[body.key]){
        state.attempts[body.key]=1;record(state,body,'failed','Mock WMS unavailable: HTTP 503; retry with same event key');save();throw Object.assign(new Error('Mock WMS unavailable; retry is safe'),{status:503});
      }
      const next=structuredClone(state);
      try{const result=applyEvent(next,body);state=next;save();return result;}
      catch(error){record(state,body,'rejected',error.message);save();throw error;}
    }
    if(req.method==='POST'&&url.pathname==='/reconcile'){reconcile(state);save();return publish();}
    throw Object.assign(new Error('Not found'),{status:404});
  };
  // Serialize mock adapter mutations and native report publication.
  const result=queue.then(run);queue=result.catch(()=>{});
  try{const data=await result;if(!res.writableEnded){res.setHeader('Content-Type','application/json');res.end(JSON.stringify(data));}}
  catch(e){if(req.url==='/publish'){state.publishError=e.message;save();}res.writeHead(e.status||500,{'Content-Type':'application/json'});res.end(JSON.stringify({error:e.message}));}
});
server.listen(8090,'0.0.0.0',()=>console.log('Inventory PoC adapter listening on 8090'));
