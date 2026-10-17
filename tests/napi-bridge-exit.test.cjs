'use strict';
const {test}=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs'),path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'../hook/napi-bridge.cjs'),'utf8');
async function fixture(pauseOnExit=true){
  const files=new Map(),events=[],handlers=new Map(),order=[];let timer,exitCode=null,controllerAlive=true;
  const config={pid:42,controllerPid:43,source:'owned-agent',fridaModule:'fixture-frida',intervalMs:5000,pauseOnExit,
    transport:{eventsFile:'C:/fixture/events',stopFile:'C:/fixture/stop',heartbeatFile:'C:/fixture/heartbeat',snapshotFile:'C:/fixture/snapshot',
      exitFile:'C:/fixture/exit',exitResultFile:'C:/fixture/exit-result',exitCommandId:'stable-exit'}};
  files.set('C:/fixture/config',JSON.stringify(config));
  const fakeFs={openSync:()=>1,closeSync(){},writeSync:(_,value)=>events.push(JSON.parse(value)),existsSync:p=>files.has(p),
    readFileSync:p=>{if(!files.has(p))throw new Error('missing');return files.get(p);},
    writeFileSync:(p,value)=>{
      files.set(p,value);
      if(p===config.transport.exitFile){order.push('pause-request');files.set(config.transport.exitResultFile,JSON.stringify({id:'stable-exit',ok:true,paused:true,secret:'SECRET'}));}
      if(p===config.transport.stopFile)order.push('stop-marker');
    }};
  const script={logHandler:null,message:{connect(){}},load:async()=>{},unload:async()=>order.push('unload'),exports:{snapshot:async()=>({}),stop:async()=>{order.push('cleanup');return{cleaned:true};}}};
  const session={detached:{connect(){}},createScript:async()=>script,detach:async()=>order.push('detach')};
  const fakeProcess={argv:['node','bridge','C:/fixture/config'],on:(name,fn)=>handlers.set(name,fn),exit:code=>{exitCode=code;},
    stderr:{write(){}},kill:()=>{if(!controllerAlive)throw Object.assign(new Error('gone'),{code:'ESRCH'});}};
  const context={process:fakeProcess,require:name=>name==='node:fs'?fakeFs:name==='node:path'?path:{getLocalDevice:async()=>({attach:async()=>session})},
    setInterval:fn=>{timer=fn;return 1;},clearInterval(){},setTimeout,clearTimeout};
  vm.runInNewContext(source,context);
  for(let i=0;i<3;i++)await new Promise(setImmediate);
  return{events,order,files,config,signal:()=>handlers.get('SIGTERM')(),gone:()=>{controllerAlive=false;timer();},
    stop:()=>{files.set(config.transport.stopFile,'stop');timer();},done:async()=>{for(let i=0;i<5&&exitCode===null;i++)await new Promise(setImmediate);return exitCode;}};
}
test('helper signal pauses before writing teardown marker and sanitizes pause result',async()=>{
  const f=await fixture();f.signal();assert.equal(await f.done(),0);
  assert.deepEqual(f.order,['pause-request','stop-marker','cleanup','unload','detach']);
  const result=f.events.find(x=>x.kind==='exit_pause').result;assert.equal(result.paused,true);assert.equal(JSON.stringify(result).includes('SECRET'),false);
});
test('controller death uses exit lane, while ordinary teardown never changes billing',async()=>{
  const dead=await fixture();dead.gone();assert.equal(await dead.done(),0);assert.equal(dead.order[0],'pause-request');
  const reconnect=await fixture();reconnect.stop();assert.equal(await reconnect.done(),0);assert.equal(reconnect.order.includes('pause-request'),false);
});
test('read-only helper exits without issuing a pause',async()=>{
  const f=await fixture(false);f.signal();assert.equal(await f.done(),0);assert.equal(f.order.includes('pause-request'),false);
});
