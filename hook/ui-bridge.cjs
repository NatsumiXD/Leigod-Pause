'use strict';
const readline=require('node:readline');
const input=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
let config, stopping=false, socket, timer, generation=0, nextId=1;
const pending=new Map();
function emit(v){if(!process.stdout.destroyed)process.stdout.write(JSON.stringify(v)+'\n');}
function failPending(){for(const {reject,timer} of pending.values()){clearTimeout(timer);reject(new Error('CDP disconnected'));}pending.clear();}
function call(method,params={}){
  return new Promise((resolve,reject)=>{
    if(!socket||socket.readyState!==WebSocket.OPEN)return reject(new Error('CDP not connected'));
    const id=nextId++;
    const timer=setTimeout(()=>{pending.delete(id);reject(new Error(method+' timed out'));},4000);
    pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));
  });
}
async function evaluate(expression){
  const result=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});
  if(result.exceptionDetails)throw new Error(result.exceptionDetails.text||'Renderer evaluation failed');
  return result.result?.value;
}
async function connect(){
  const resourcePath=config.resourcePath.replace(/\\/g,'/').toLowerCase();
  const ports=[config.port];
  try {
    if(typeof config.activePortFile==='string') {
      const discovered=Number(require('node:fs').readFileSync(config.activePortFile,'utf8').split(/\r?\n/)[0].trim());
      if(Number.isInteger(discovered)&&discovered>=1024&&discovered<=65535)ports.push(discovered);
    }
  }catch(_){}
  ports.push(9222);
  let target=null;
  for(const port of new Set(ports)) {
    try {
      const response=await fetch('http://127.0.0.1:'+port+'/json/list',{signal:AbortSignal.timeout(1200)});
      if(!response.ok)continue;
      const targets=await response.json();
      target=targets.find(t=>t.type==='page'&&typeof t.url==='string'&&
        decodeURI(t.url).replace(/\\/g,'/').toLowerCase().includes(resourcePath+'/renderer.asar/')&&t.webSocketDebuggerUrl);
      if(target)break;
    }catch(_){}
  }
  if(!target)throw new Error('LeiGod local debugging endpoint unavailable');
  const url=new URL(target.webSocketDebuggerUrl);
  if(!['127.0.0.1','localhost','[::1]'].includes(url.hostname)||url.protocol!=='ws:')throw new Error('CDP endpoint must be local');
  const current=++generation;
  socket=new WebSocket(url);
  await new Promise((resolve,reject)=>{
    const timeout=setTimeout(()=>reject(new Error('CDP WebSocket connect timed out')),3000);
    socket.addEventListener('open',()=>{clearTimeout(timeout);resolve();},{once:true});
    socket.addEventListener('error',()=>{clearTimeout(timeout);reject(new Error('CDP WebSocket failed'));},{once:true});
  });
  socket.addEventListener('message',event=>{
    let reply;try{reply=JSON.parse(String(event.data));}catch(_){return;}
    const request=pending.get(reply.id);if(!request)return;
    pending.delete(reply.id);clearTimeout(request.timer);
    if(reply.error)request.reject(new Error(reply.error.message));else request.resolve(reply.result);
  });
  socket.addEventListener('close',()=>{if(current===generation){failPending();socket=null;}});
}
async function poll(){
  if(stopping)return;
  try{
    if(!socket||socket.readyState!==WebSocket.OPEN)await connect();
    const installed=await evaluate(config.source);
    if(!installed)throw new Error('Acceleration store not ready');
    const snapshot=await evaluate('window['+JSON.stringify(config.key)+'].snapshot()');
    if(!stopping)emit({kind:'snapshot',snapshot});
  }catch(error){
    if(!stopping)emit({kind:'snapshot',snapshot:{available:false,state:'unknown',error:error.message}});
  }
  if(!stopping)timer=setTimeout(poll,config.intervalMs);
}
async function stop(){
  if(stopping)return;stopping=true;clearTimeout(timer);
  const deadline=setTimeout(()=>process.exit(0),2000);
  try{if(socket?.readyState===WebSocket.OPEN)await evaluate('window['+JSON.stringify(config.key)+']?.dispose()');}catch(_){}
  socket?.close();failPending();clearTimeout(deadline);process.exit(0);
}
input.on('line',line=>{
  try{
    const value=JSON.parse(line);
    if(config){if(value.command==='stop')void stop();return;}
    if(!Number.isInteger(value.port)||value.port<1024||value.port>65535||typeof value.source!=='string'||typeof value.resourcePath!=='string'||
      typeof value.key!=='string'||!/^__leigodReadOnlyMonitor_\w+$/.test(value.key)||value.intervalMs<100)throw new Error('Invalid UI config');
    config=value;void poll();
  }catch(error){emit({kind:'error',error:error.message});void stop();}
});
input.on('close',()=>void stop());process.on('SIGINT',()=>void stop());process.on('SIGTERM',()=>void stop());
process.stdout.on('error',()=>void stop());
