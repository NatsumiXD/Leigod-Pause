'use strict';
const {test}=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs'),path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'../hook/main-agent.js'),'utf8');
async function fixture(withTransport=false,withCommands=false,pauseOnExit=false,fastTimeout=false){
  let callback,commandCallback,cleared=false,hang=false,command=null,result=null,controls=0;
  let exitCommand=null,exitResult=null,quitListener=null,quits=0,controlHandler=null;
  const actions=[];
  let pageUrl='file:///C:/Fixture/resources/renderer.asar/index.html';
  const startResult=Promise.resolve({result:{code:200}}),stopResult=Promise.resolve({result:{code:200}});
  const start=function(...args){start.args=args;return startResult;},stop=()=>stopResult;
  const handlers=vm.runInNewContext('new Map()');handlers.set('start-acc',start);handlers.set('stop-acc',stop);
  const ui={available:true,state:'inactive',game:null};
  const page={isDestroyed:()=>false,getURL:()=>pageUrl,
    executeJavaScript:async expression=>{
      if(hang)return new Promise(()=>{});
      if(expression.includes('?.control(')){controls++;const command=JSON.parse(expression.slice(expression.indexOf('?.control(')+10,-1));actions.push(command.action);if(controlHandler)return controlHandler(command);return{id:command.id,ok:true,stopped:true,paused:true,secret:'MUST_NOT_EXPORT'};}
      return expression==='install-ui'?true:expression.includes('dispose')?true:{...ui};
    }};
  const electron={app:{on:(name,fn)=>{quitListener=fn;},removeListener:()=>{quitListener=null;},quit:()=>{quits++;}},ipcMain:{_invokeHandlers:handlers},BrowserWindow:{getAllWindows:()=>[{isDestroyed:()=>false,getSize:()=>[1020,640],webContents:page}]}};
  const timers={setInterval:(fn,ms)=>{if(ms===150)commandCallback=fn;else callback=fn;return{unref(){}};},clearInterval:()=>{cleared=true;},setTimeout:(fn,ms)=>setTimeout(fn,fastTimeout?Math.min(ms,5):ms),clearTimeout};
  let stopped=false,published=null;
  const fakeFs={existsSync:p=>p==='commands'?command!==null:p==='exit'?exitCommand!==null:stopped,statSync:()=>({mtimeMs:Date.now()}),
    readFileSync:p=>JSON.stringify(p==='exit'?exitCommand:command),writeFileSync:(p,value)=>{if(p==='results')result=JSON.parse(value);else if(p==='exit-results')exitResult=JSON.parse(value);else published=JSON.parse(value);}};
  const context=vm.createContext({Map,process:{mainModule:{require:name=>name==='electron'?electron:name==='node:fs'?fakeFs:timers}},
    LEIGOD_MAIN_TRANSPORT:withTransport?{stopFile:'stop',heartbeatFile:'heartbeat',snapshotFile:'snapshot',...(withCommands?{commandFile:'commands',resultFile:'results'}:{}),...(pauseOnExit?{exitFile:'exit',exitResultFile:'exit-results',exitCommandId:'exit-fixed'}:{})}:undefined,
    LEIGOD_MAIN_CONFIG:{key:'testObserver',uiKey:'testUI',uiSource:'install-ui',resourcePath:'C:/Fixture/resources',intervalMs:1000,pauseOnExit}});
  vm.runInContext(source,context);await new Promise(setImmediate);
  return{handlers,start,stop,startResult,stopResult,ui,observer:context.testObserver,tick:async()=>{callback();await new Promise(setImmediate);},cleared:()=>cleared,published:()=>published,stopTransport:()=>{stopped=true;},hang:()=>{hang=true;},
    setCommand:value=>command=value,controls:()=>controls,result:()=>result,setUrl:value=>pageUrl=value,
    commandTick:async()=>{commandCallback();await new Promise(setImmediate);},setExit:value=>exitCommand=value,exitResult:()=>exitResult,actions,
    setControlHandler:fn=>controlHandler=fn,quits:()=>quits,beforeQuit:()=>{let prevented=false;quitListener?.({preventDefault(){prevented=true;}});return prevented;}};
}
test('IPC wrappers preserve arguments, this, original Promise identity and confirmed states',async()=>{
  const f=await fixture();assert.equal(f.observer.snapshot().state,'inactive');
  const event={sender:1},args={game_id:42,game_name:'Fixture game'};
  const result=f.handlers.get('start-acc')(event,args);assert.equal(result,f.startResult);assert.equal(f.start.args[0],event);assert.equal(f.start.args[1],args);
  f.ui.state='active';f.ui.game={id:'42',name:'Fixture game'};await result;await f.tick();
  let s=f.observer.snapshot();assert.equal(s.state,'active');assert.equal(s.backend_state,'active');assert.equal(s.ipc_hook_available,true);
  assert.equal(s.last_ipc_action.code,200);
  assert.equal(f.handlers.get('stop-acc')(),f.stopResult);f.ui.state='inactive';f.ui.game=null;await f.stopResult;await f.tick();
  s=f.observer.snapshot();assert.equal(s.state,'inactive');assert.equal(s.backend_state,'inactive');
  f.observer.dispose();assert.equal(f.handlers.get('start-acc'),f.start);assert.equal(f.handlers.get('stop-acc'),f.stop);assert.equal(f.cleared(),true);
});

test('independent command timer dispatches exactly once and publishes only sanitized result fields',async()=>{
  const f=await fixture(true,true);
  f.setCommand({id:'one',action:'stop_and_pause',expected_game_id:'42',expected_session:'session:1',expires_at_ms:Date.now()+30000});
  await f.commandTick();assert.equal(f.controls(),1);assert.equal(f.result().ok,true);assert.equal(f.result().paused,true);
  assert.equal(JSON.stringify(f.result()).includes('MUST_NOT_EXPORT'),false);
  assert.equal(f.observer.snapshot().control_result.id,'one');
  await f.commandTick();assert.equal(f.controls(),1);f.observer.dispose();
});

test('expired commands and untrusted navigations never execute renderer control',async()=>{
  const f=await fixture(true,true);
  f.setCommand({id:'expired',action:'stop_and_pause',expected_game_id:'42',expected_session:'s',expires_at_ms:Date.now()-1});
  await f.commandTick();assert.equal(f.result().error,'expired');assert.equal(f.controls(),0);
  f.setUrl('https://example.invalid/?file:///C:/Fixture/resources/renderer.asar/index.html');
  f.setCommand({id:'untrusted',action:'stop_and_pause',expected_game_id:'42',expected_session:'s',expires_at_ms:Date.now()+30000});
  await f.commandTick();assert.equal(f.result().error,'renderer_unavailable');assert.equal(f.controls(),0);f.observer.dispose();
});
test('conflicting UI and completed IPC state becomes unknown',async()=>{
  const f=await fixture();await f.handlers.get('start-acc')({}, {game_id:42});
  assert.equal(f.observer.snapshot().state,'unknown');f.observer.dispose();
});
test('main-thread timer publishes fresh state without further addon callbacks and disposes on stop marker',async()=>{
  const f=await fixture(true);
  assert.equal(f.published().snapshot.state,'inactive');
  f.ui.state='active';f.ui.game={id:'42',name:'Fixture'};await f.tick();
  assert.equal(f.published().snapshot.state,'active');
  assert.equal(f.published().installed,true);
  f.stopTransport();await f.tick();
  assert.equal(f.cleared(),true);assert.equal(f.handlers.get('start-acc'),f.start);
});
test('stalled renderer cannot prevent main timer and IPC wrapper cleanup',async()=>{
  const f=await fixture(true);f.hang();await f.tick();f.stopTransport();await f.tick();
  assert.equal(f.cleared(),true);assert.equal(f.handlers.get('start-acc'),f.start);
});
test('exit lane pauses once and publishes independent sanitized results while an automatic stop is busy',async()=>{
  const f=await fixture(true,true,true);let resolveNormal;
  f.setControlHandler(command=>command.action==='stop_and_pause'?new Promise(resolve=>resolveNormal=()=>resolve({id:command.id,ok:false,error:'exit_requested'})):
    {id:command.id,ok:true,paused:true,secret:'MUST_NOT_EXPORT'});
  f.setCommand({id:'normal',action:'stop_and_pause',expected_game_id:'42',expected_session:'s',expires_at_ms:Date.now()+30000});await f.commandTick();
  f.setExit({id:'exit-fixed',action:'pause_before_exit',expires_at_ms:Date.now()+2000});await f.commandTick();
  assert.deepEqual(f.actions,['stop_and_pause','pause_before_exit']);assert.equal(f.exitResult().paused,true);assert.equal(JSON.stringify(f.exitResult()).includes('SECRET'),false);
  await f.commandTick();assert.equal(f.controls(),2);resolveNormal();await new Promise(setImmediate);f.observer.dispose();
});
test('before-quit is bounded, coalesced and resumes quit only after pausing',async()=>{
  const f=await fixture(true,true,true);let resolvePause;
  f.setControlHandler(command=>new Promise(resolve=>resolvePause=()=>resolve({id:command.id,ok:true,paused:true})));
  assert.equal(f.beforeQuit(),true);assert.equal(f.beforeQuit(),true);assert.equal(f.quits(),0);
  await new Promise(setImmediate);resolvePause();await new Promise(setImmediate);
  assert.equal(f.quits(),1);assert.equal(f.controls(),1);assert.equal(f.exitResult().paused,true);f.observer.dispose();
});
test('read-only mode does not intercept quit or dispatch exit commands',async()=>{
  const f=await fixture(true,true,false);assert.equal(f.beforeQuit(),false);assert.equal(f.controls(),0);f.observer.dispose();
});
test('a stalled renderer cannot hold application quit beyond the exit budget',async()=>{
  const f=await fixture(true,true,true,true);f.hang();assert.equal(f.beforeQuit(),true);
  await new Promise(resolve=>setTimeout(resolve,20));assert.equal(f.quits(),1);assert.equal(f.exitResult().error,'pause_timeout');f.observer.dispose();
});
test('expired exit requests never pause, and report failure on the exit channel',async()=>{
  const f=await fixture(true,true,true);
  f.setExit({id:'exit-fixed',action:'pause_before_exit',expires_at_ms:Date.now()-1});await f.commandTick();
  assert.equal(f.exitResult().error,'expired');assert.equal(f.controls(),0);f.observer.dispose();
});
