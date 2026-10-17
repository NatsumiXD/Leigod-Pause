'use strict';
const {test}=require('node:test');
const assert=require('node:assert/strict');
const vm=require('node:vm');
const fs=require('node:fs');
const source=fs.readFileSync(require('node:path').join(__dirname,'../hook/ui-agent.js'),'utf8');
function fixture(storeName='acc'){
  let callback,disposed=false;
  const info={accStatus:'normal',game_id:'',getAccParams:null};
  const store={accInfo:info,startAcc(){},stopAcc(){},$onAction(cb){callback=cb;return()=>{disposed=true;};}};
  const stores=new Map([[storeName,store]]);
  const sandbox={Map,window:{},setTimeout,clearTimeout,document:{getElementById:()=>({__vue_app__:{config:{globalProperties:{$pinia:{_s:stores}}}}})}};
  const installed=vm.runInNewContext(source,sandbox);
  return{info,store,stores,sandbox,installed,monitor:sandbox.window.__leigodReadOnlyMonitor_v1,action(name){const result={};callback({name,after:fn=>result.after=fn,onError:fn=>result.onError=fn});return result;},disposed:()=>disposed};
}
test('discovers renamed store by capabilities and installs only once',()=>{
  const f=fixture('acc-vNext');assert.equal(f.installed,true);const first=f.monitor;
  assert.equal(vm.runInNewContext(source,f.sandbox),true);assert.equal(f.sandbox.window.__leigodReadOnlyMonitor_v1,first);
  first.dispose();assert.equal(f.disposed(),true);assert.equal(f.sandbox.window.__leigodReadOnlyMonitor_v1,undefined);
});

async function controlFixture(paused=false){
  const f=fixture();let stops=0,pauses=0;
  Object.assign(f.info,{accStatus:'speeding',game_id:42,getAccParams:()=>({gameInfo:{title:'Test',game_process:'game.exe'}})});
  const account={isLogin:true,userInfo:{},userTimeInfo:{timeStatus:paused?'pause':'timeing'},progressing:false,
    async toggleTimeStatus(direction,options){assert.equal(direction,'pause');assert.ok(['leigod_status_auto_stop','leigod_status_exit'].includes(options.scene));pauses++;this.userTimeInfo.timeStatus='pause';}};
  f.stores.set('user',account);
  f.store.stopAcc=async options=>{assert.equal(String(options.game_id),'42');assert.equal(options.isConfirm,false);stops++;f.info.accStatus='normal';f.info.game_id='';};
  const snapshot=await f.monitor.snapshot();
  const command={id:'command-1',action:'stop_and_pause',expected_game_id:'42',expected_session:snapshot.game.session,expires_at_ms:Date.now()+60000};
  return{...f,account,command,stops:()=>stops,pauses:()=>pauses};
}

test('authorized stop and explicit pause verify both statuses, with replay prevented',async()=>{
  const f=await controlFixture();const result=await f.monitor.control(f.command);
  assert.equal(result.ok,true);assert.equal(result.stopped,true);assert.equal(result.paused,true);
  assert.equal(f.stops(),1);assert.equal(f.pauses(),1);
  assert.equal((await f.monitor.control(f.command)).error,'replay');assert.equal(f.stops(),1);
});

test('changed game or same game restarted invalidates session before any action',async()=>{
  for(const restart of [false,true]){
    const f=await controlFixture();if(restart)f.action('startAcc');else f.info.game_id=43;
    assert.equal((await f.monitor.control(f.command)).error,'game_changed');assert.equal(f.stops(),0);assert.equal(f.pauses(),0);
  }
});

test('expired commands do nothing and already-paused accounts are never resumed',async()=>{
  const expired=await controlFixture();expired.command.expires_at_ms=Date.now()-1;
  assert.equal((await expired.monitor.control(expired.command)).error,'expired');assert.equal(expired.stops(),0);
  const paused=await controlFixture(true);assert.equal((await paused.monitor.control(paused.command)).ok,true);
  assert.equal(paused.stops(),1);assert.equal(paused.pauses(),0);
});

test('stop/pause failures are sanitized and never blindly retried',async()=>{
  const stop=await controlFixture();stop.store.stopAcc=()=>Promise.reject(new Error('SECRET_TOKEN'));
  let result=await stop.monitor.control(stop.command);assert.equal(result.error,'stop_failed');assert.equal(stop.pauses(),0);
  assert.equal(JSON.stringify(result).includes('SECRET'),false);
  const pause=await controlFixture();pause.account.toggleTimeStatus=()=>Promise.reject(new Error('SECRET_TOKEN'));
  result=await pause.monitor.control(pause.command);assert.equal(result.error,'pause_failed');assert.equal(result.stopped,true);assert.equal(result.paused,false);
});

test('restart while stop is pending prevents pause; hung stop reports timeout',async()=>{
  const changed=await controlFixture();changed.store.stopAcc=async()=>{changed.info.accStatus='normal';changed.info.game_id='';changed.action('startAcc');};
  assert.equal((await changed.monitor.control(changed.command)).error,'game_changed');assert.equal(changed.pauses(),0);
  const hung=await controlFixture();hung.store.stopAcc=()=>new Promise(()=>{});
  hung.sandbox.setTimeout=(fn,ms)=>setTimeout(fn,Math.min(ms,5));
  assert.equal((await hung.monitor.control(hung.command)).error,'stop_timeout');assert.equal(hung.pauses(),0);
});

test('dispatch expiry does not interrupt an accepted stop/pause transaction',async()=>{
  const f=await controlFixture();let now=Date.now();f.sandbox.Date={now:()=>now};
  f.command.expires_at_ms=now+2000;
  f.store.stopAcc=async()=>{now+=3000;f.info.accStatus='normal';f.info.game_id='';};
  const result=await f.monitor.control(f.command);assert.equal(result.ok,true);assert.equal(f.pauses(),1);
});
test('switching accounts inside the same Pinia store prevents pausing the new account',async()=>{
  const f=await controlFixture();f.account.userInfo.nn_id='first';
  f.store.stopAcc=async()=>{f.info.accStatus='normal';f.info.game_id='';f.account.userInfo.nn_id='second';};
  assert.equal((await f.monitor.control(f.command)).error,'account_changed');assert.equal(f.pauses(),0);
});
test('start click alone does not mean acceleration succeeded; live status supplies game',async()=>{
  const f=fixture();const action=f.action('startAcc');
  let snap=await f.monitor.snapshot();assert.equal(snap.state,'inactive');assert.equal(snap.last_action.name,'startAcc');
  f.info.accStatus='loading';snap=await f.monitor.snapshot();assert.equal(snap.state,'unknown');
  f.info.accStatus='speeding';f.info.game_id=42;f.info.getAccParams=()=>({gameInfo:{title:'Test game',game_process:'game.exe, child.exe',token:'DO_NOT_EXPORT'},token:'DO_NOT_EXPORT'});
  action.after();snap=await f.monitor.snapshot();assert.equal(snap.state,'active');assert.equal(snap.game.name,'Test game');
  assert.deepEqual(Array.from(snap.game.configured_process_names),['game.exe','child.exe']);assert.equal(JSON.stringify(snap).includes('DO_NOT_EXPORT'),false);
});
test('cancelled stop preserves active state; confirmed stop clears game',async()=>{
  const f=fixture();Object.assign(f.info,{accStatus:'speeding',game_id:42,getAccParams:()=>({gameInfo:{title:'Test',game_process:'game.exe'}})});
  f.action('stopAcc').after();assert.equal((await f.monitor.snapshot()).state,'active');
  Object.assign(f.info,{accStatus:'normal',game_id:'',getAccParams:null});const stopped=await f.monitor.snapshot();
  assert.equal(stopped.state,'inactive');assert.equal(stopped.game,null);
});
test('user store is discovered by shape, exports allowlisted time/profile and observes changes',async()=>{
  const f=fixture();let notify,disposed=false;
  const user={isLogin:true,userInfo:{nn_number:'10001',nickname:'Fixture user',payStatus:2,account_token:'SECRET_TOKEN',password:'SECRET_PASSWORD'},
    userTimeInfo:{timeStatus:'pause',totalTimeLeft:3600,canPauseTimeLeft:3000,notCanPauseTimeLeft:600,token:'SECRET_TIME_TOKEN'},
    viewTimeStatus:'paused',is_returning_user:false,$subscribe:fn=>{notify=fn;return()=>{disposed=true;};}};
  f.stores.set('renamed-user-vNext',user);
  const first=await f.monitor.snapshot();assert.equal(first.user_hook_available,true);assert.equal(first.user.nickname,'Fixture user');
  assert.equal(first.user.time.paused,true);assert.equal(first.user.time.total_remaining_seconds,3600);
  assert.equal(JSON.stringify(first).includes('SECRET'),false);
  notify();assert.equal((await f.monitor.snapshot()).last_user_event.kind,'changed');
  f.monitor.dispose();assert.equal(disposed,true);
});
test('logout clears stale account/time without inspecting credentials; missing store becomes unavailable',async()=>{
  const f=fixture();const user={isLogin:false,userInfo:{nn_number:'OLD_ACCOUNT',nickname:'OLD_NICKNAME'},userTimeInfo:{totalTimeLeft:99}};
  Object.defineProperty(user.userInfo,'account_token',{get(){throw new Error('Credential must never be read');}});
  f.stores.set('any-user-id',user);
  let snapshot=await f.monitor.snapshot();assert.equal(snapshot.user.logged_in,false);assert.equal(snapshot.user.account_number,null);assert.equal(snapshot.user.time,null);
  assert.equal(JSON.stringify(snapshot).includes('OLD_'),false);
  user.isLogin=true;snapshot=await f.monitor.snapshot();assert.equal(snapshot.user.account_number,'OLD_ACCOUNT');
  f.stores.delete('any-user-id');snapshot=await f.monitor.snapshot();assert.equal(snapshot.user,null);assert.equal(snapshot.user_hook_available,false);
});
const exitCommand=id=>({id,action:'pause_before_exit',expires_at_ms:Date.now()+2000});
test('exit pauses immediately while accelerating, never stops a game, and caches repeated requests',async()=>{
  const f=await controlFixture();const command=exitCommand('exit-1');
  const first=await f.monitor.control(command),second=await f.monitor.control(command);
  assert.equal(first.ok,true);assert.equal(first.paused,true);assert.equal(first,second);
  assert.equal(f.stops(),0);assert.equal(f.pauses(),1);assert.equal(f.info.accStatus,'speeding');
  assert.equal((await f.monitor.control(f.command)).error,'exit_requested');
});
test('exit never resumes a paused account and ignores expired or logged-out requests',async()=>{
  const paused=await controlFixture(true);assert.equal((await paused.monitor.control(exitCommand('paused'))).ok,true);assert.equal(paused.pauses(),0);
  const expired=await controlFixture();const command=exitCommand('expired');command.expires_at_ms=Date.now()-1;
  assert.equal((await expired.monitor.control(command)).error,'expired');assert.equal(expired.pauses(),0);
  const loggedOut=await controlFixture();loggedOut.account.isLogin=false;
  assert.equal((await loggedOut.monitor.control(exitCommand('out'))).error,'pause_unavailable');assert.equal(loggedOut.pauses(),0);
});
test('exit preempts automatic stop at its next safe point and prevents another pause',async()=>{
  const f=await controlFixture();let finishStop;
  f.store.stopAcc=()=>new Promise(resolve=>{finishStop=()=>{f.info.accStatus='normal';f.info.game_id='';resolve();};});
  const ordinary=f.monitor.control(f.command);await new Promise(setImmediate);
  const exit=await f.monitor.control(exitCommand('preempt'));
  assert.equal(exit.ok,true);finishStop();assert.equal((await ordinary).error,'exit_requested');assert.equal(f.pauses(),1);
});
test('exit waits for an existing pause without toggling twice and rejects account switches',async()=>{
  for(const switchAccount of [false,true]){
    const f=await controlFixture();let finishPause,pauses=0;
    f.account.userInfo.nn_id='old';
    f.account.toggleTimeStatus=()=>{pauses++;return new Promise(resolve=>{finishPause=()=>{f.account.userTimeInfo.timeStatus='pause';resolve();};});};
    const ordinary=f.monitor.control(f.command);await new Promise(setImmediate);
    const exit=f.monitor.control(exitCommand('same-flight'));
    if(switchAccount)f.account.userInfo.nn_id='new';
    finishPause();const result=await exit;await ordinary;
    assert.equal(pauses,1);assert.equal(result.ok,!switchAccount);
    if(switchAccount)assert.equal(result.error,'account_changed');
  }
});
test('timed out exit does not submit a late second pause when an old billing call eventually finishes',async()=>{
  const f=await controlFixture();let finishPause,calls=0;
  f.account.toggleTimeStatus=()=>{calls++;return new Promise(resolve=>finishPause=resolve);};
  const ordinary=f.monitor.control(f.command);await new Promise(setImmediate);
  f.sandbox.setTimeout=(fn,ms)=>setTimeout(fn,Math.min(ms,5));
  const exit=await f.monitor.control(exitCommand('timeout'));
  assert.equal(exit.error,'pause_timeout');finishPause();await ordinary;await new Promise(setImmediate);
  assert.equal(calls,1);
});
test('exit waits for a manual resume before pausing instead of trusting an old paused snapshot',async()=>{
  const f=await controlFixture(true);f.account.progressing=true;
  const exit=f.monitor.control(exitCommand('manual-resume'));
  await new Promise(setImmediate);assert.equal(f.pauses(),0);
  f.account.userTimeInfo.timeStatus='timeing';f.account.progressing=false;
  const result=await exit;assert.equal(result.ok,true);assert.equal(f.pauses(),1);assert.equal(f.account.userTimeInfo.timeStatus,'pause');
});
