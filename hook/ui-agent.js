// Evaluated in LeiGod's renderer through Electron webContents or localhost CDP.
// Pinia action subscriptions observe the same actions called by start/stop UI.
(() => {
  const key = '__leigodReadOnlyMonitor_v1';
  if (window[key]?.control_version===2) return true;
  window[key]?.dispose?.();
  const root = document.getElementById('app') || document.querySelector('[data-v-app]');
  const app = root?.__vue_app__;
  const context = app?._context || root?.__vueParentComponent?.appContext;
  let pinia = app?.config?.globalProperties?.$pinia;
  if (!pinia && context?.provides) {
    for (const k of Reflect.ownKeys(context.provides)) {
      const value = context.provides[k];
      if (value?._s instanceof Map) { pinia = value; break; }
    }
  }
  const store = Array.from(pinia?._s?.values() || []).find(s => s?.accInfo &&
    typeof s.startAcc === 'function' && typeof s.stopAcc === 'function');
  if (!store || typeof store.$onAction !== 'function') return false;
  let lastAction = null;
  let disposed=false,sessionVersion=0,trackedId=String(store.accInfo?.game_id||'');
  const sessionTag=Date.now().toString(36)+'-'+Math.random().toString(36).slice(2);
  const commandIds=new Set();
  let pendingControl=null,lastControlResult=null;
  let exitRequested=false,billingFlight=null;
  const exitCommands=new Map();
  function session(){
    const id=String(store.accInfo?.game_id||'');
    if(id&&id!==trackedId)sessionVersion++;
    trackedId=id;
    return sessionTag+':'+sessionVersion;
  }
  let dbCache = null;
  let userStore=null,userUnsubscribe=null,lastUserEvent=null;
  function findUserStore(){
    const candidate=Array.from(pinia?._s?.values()||[]).find(s=>s&&'userInfo' in s&&'userTimeInfo' in s&&'isLogin' in s);
    if(candidate!==userStore){
      userUnsubscribe?.();userUnsubscribe=null;userStore=candidate||null;lastUserEvent=null;
      if(typeof userStore?.$subscribe==='function'){
        userUnsubscribe=userStore.$subscribe(()=>{lastUserEvent={kind:'changed',time_ms:Date.now()};},{detached:true,flush:'sync'});
      }
    }
    return userStore;
  }
  const text=value=>typeof value==='string'?value.slice(0,256):typeof value==='number'&&Number.isFinite(value)?String(value):null;
  const seconds=value=>typeof value==='number'&&Number.isFinite(value)&&value>=0?value:null;
  function userSnapshot(){
    try{
      const account=findUserStore();
      if(!account)return{user:null,user_hook_available:false,user_error:'User store not ready',last_user_event:null};
      const logged=typeof account.isLogin==='boolean'?account.isLogin:null;
      // Strict allowlist: never enumerate/export userInfo or authentication data.
      const info=logged===true?account.userInfo:null,time=logged===true?account.userTimeInfo:null;
      const status=text(time?.timeStatus);
      const loaded=status==='pause'||status==='timeing';
      return{user:{logged_in:logged,account_number:text(info?.nn_number),nn_id:text(info?.nn_id),nickname:text(info?.nickname),
        pay_status:typeof info?.payStatus==='number'?info.payStatus:null,
        is_new_today:logged===true&&typeof info?.isTodayUser==='boolean'?info.isTodayUser:null,
        is_returning_user:logged===true&&typeof account.is_returning_user==='boolean'?account.is_returning_user:null,
        time:logged===true?{loaded,status,view_status:text(account.viewTimeStatus),paused:status==='pause'?true:status==='timeing'?false:null,
          operation_pending:typeof account.progressing==='boolean'?account.progressing:null,
          refreshing:typeof account.refreshing==='boolean'?account.refreshing:null,
          auto_use_pausable:[0,1].includes(info?.is_use_pause_time)?info.is_use_pause_time===1:null,
          total_remaining_seconds:loaded?seconds(time?.totalTimeLeft):null,pausable_remaining_seconds:loaded?seconds(time?.canPauseTimeLeft):null,
          nonpausable_remaining_seconds:loaded?seconds(time?.notCanPauseTimeLeft):null}:null},
        user_hook_available:true,user_error:null,last_user_event:lastUserEvent};
    }catch(_){return{user:null,user_hook_available:false,user_error:'User snapshot unavailable',last_user_event:null};}
  }
  const unsubscribe = store.$onAction(({name, after, onError}) => {
    if (!['startAcc', 'stopAcc'].includes(name)) return;
    if(name==='startAcc')sessionVersion++;
    const action = {name, phase:'requested', time_ms:Date.now()};
    lastAction = action;
    after(() => { action.phase = 'returned'; });
    onError(() => { action.phase = 'failed'; });
  }, true);
  function names(value) {
    if (Array.isArray(value)) return [...new Set(value.flatMap(names))];
    if (typeof value !== 'string') return [];
    if(value.trim().startsWith('[')) {
      try { const decoded=JSON.parse(value); if(Array.isArray(decoded))return names(decoded); } catch (_) {}
    }
    return [...new Set(value.split(/[,;|\r\n]+/).map(s => s.trim().replace(/^"|"$/g, '')).filter(s => s && s.length <= 512))];
  }
  async function localGame(id) {
    if (dbCache?.id === String(id)) return dbCache.value;
    const existing = await indexedDB.databases();
    const dbName=existing.filter(x=>/^leigod_database_/i.test(x.name||'')).sort((a,b)=>b.name.localeCompare(a.name,undefined,{numeric:true}))[0]?.name;
    if (!dbName) return null;
    const db = await new Promise((resolve,reject) => {
      const request=indexedDB.open(dbName);
      request.onupgradeneeded=()=>{ request.transaction.abort(); reject(new Error('Database missing')); };
      request.onerror=()=>reject(request.error); request.onsuccess=()=>resolve(request.result);
    });
    try {
      if(!db.objectStoreNames.contains('game_list'))return null;
      let value=null;
      for(const candidate of [Number(id),String(id)]) {
        if(typeof candidate==='number'&&!Number.isFinite(candidate))continue;
        value=await new Promise((resolve,reject)=>{
          const table=db.transaction('game_list','readonly').objectStore('game_list');
          const request=table.indexNames.contains('id')?table.index('id').get(candidate):table.get(candidate);
          request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);
        });
        if(value)break;
      }
      // Export only game identity and process configuration, never whole rows.
      const filtered=value?{name:value.title||value.game_name||null, processes:names(value.game_process)}:null;
      dbCache={id:String(id),value:filtered};return filtered;
    } finally { db.close(); }
  }
  // Both automatic stop and exit use this single flight. An unresolved vendor
  // pause is never followed by a second toggle, including after our timeout.
  function pauseAccount(account,scene,guard){
    if(!billingFlight){
      const operation=Promise.resolve().then(()=>{guard();return account.toggleTimeStatus('pause',{scene});});
      billingFlight=operation;
      operation.then(()=>{if(billingFlight===operation)billingFlight=null;},()=>{if(billingFlight===operation)billingFlight=null;});
    }
    return billingFlight;
  }
  function exitControl(command){
    const id=typeof command?.id==='string'?command.id:null;
    if(!id||id.length>128)return Promise.resolve({id,ok:false,paused:false,error:'invalid_command',completed_at_ms:Date.now()});
    if(exitCommands.has(id))return exitCommands.get(id);
    const operation=(async()=>{
      let paused=false,timer,finished=false;
      const fail=error=>{throw new Error(error);};
      try{
        if(disposed)fail('observer_disposed');
        if(!Number.isFinite(command.expires_at_ms)||command.expires_at_ms<=Date.now())fail('expired');
        if(exitCommands.size>=32)fail('command_capacity');
        exitRequested=true;
        const account=findUserStore();
        if(!account||account.isLogin!==true||typeof account.toggleTimeStatus!=='function')fail('pause_unavailable');
        const key=JSON.stringify([text(account.userInfo?.nn_id),text(account.userInfo?.nn_number)]);
        const check=()=>{
          if(finished)fail('pause_timeout');
          if(disposed)fail('observer_disposed');
          if(findUserStore()!==account||account.isLogin!==true||JSON.stringify([text(account.userInfo?.nn_id),text(account.userInfo?.nn_number)])!==key)fail('account_changed');
        };
        await Promise.race([(async()=>{
          check();
          // Let an already submitted pause complete; never toggle concurrently.
          if(billingFlight)await billingFlight;
          check();
          // A manual resume may be in flight while the cached status still
          // says pause. Wait for its safe point before deciding what to submit.
          while(account.progressing){await new Promise(resolve=>setTimeout(resolve,50));check();}
          if(account.userTimeInfo?.timeStatus!=='pause'){
            if(account.progressing||account.userTimeInfo?.timeStatus!=='timeing')fail('billing_not_ready');
            await pauseAccount(account,'leigod_status_exit',check);
          }
          check();
          while(account.userTimeInfo?.timeStatus!=='pause'){
            await new Promise(resolve=>setTimeout(resolve,50));check();
          }
          paused=true;
        })(),new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('pause_timeout')),2600);})]);
        return{id,ok:true,paused,error:null,completed_at_ms:Date.now()};
      }catch(error){
        const allowed=new Set(['observer_disposed','expired','command_capacity','pause_unavailable','account_changed','billing_not_ready','pause_timeout']);
        return{id,ok:false,paused,error:allowed.has(error?.message)?error.message:'pause_failed',completed_at_ms:Date.now()};
      }finally{finished=true;clearTimeout(timer);}
    })();
    exitCommands.set(id,operation);return operation;
  }
  const monitor = {
    control_version:2,
    async control(command){
      if(command?.action==='pause_before_exit')return exitControl(command);
      const id=typeof command?.id==='string'?command.id:null;
      const result=(ok,error,extra={})=>({id,ok,error,...extra});
      if(!id||id.length>128||command?.action!=='stop_and_pause')return result(false,'invalid_command');
      if(commandIds.has(id))return result(false,'replay');
      if(commandIds.size>=4096)return result(false,'command_capacity');
      commandIds.add(id);
      if(pendingControl)return result(false,'control_busy');
      if(exitRequested)return result(false,'exit_requested');
      const expected=String(command.expected_game_id??'');
      const expectedSession=command.expected_session;
      const expires=command.expires_at_ms;
      let stopped=false,paused=false;
      const failure=code=>{throw new Error(code);};
      // Expiry protects dispatch; once stop is issued, finish its authorized
      // pause transaction under a separate budget and continuous session checks.
      const valid=()=>{if(disposed)failure('observer_disposed');if(exitRequested)failure('exit_requested');};
      const checkExpiry=()=>{if(!Number.isFinite(expires)||Date.now()>=expires)failure('expired');};
      const deadline=Date.now()+10000;
      const checkTime=()=>{valid();if(Date.now()>=deadline)failure('timeout');};
      const unchanged=()=>{session();if(session()!==expectedSession)failure('game_changed');};
      const idle=()=>{unchanged();if(store.accInfo?.accStatus!=='normal'||store.accInfo?.game_id)failure('game_changed');};
      const bounded=async(fn,phase)=>{
        checkTime();let timer;
        try{return await Promise.race([Promise.resolve().then(fn),new Promise((_,reject)=>{
          timer=setTimeout(()=>reject(new Error(phase+'_timeout')),Math.max(1,Math.min(4000,deadline-Date.now())));
        })]);}catch(error){if([phase+'_timeout','expired','timeout','game_changed','account_changed','observer_disposed','exit_requested'].includes(error?.message))throw error;failure(phase+'_failed');}
        finally{clearTimeout(timer);}
      };
      const wait=async(predicate,phase)=>{
        while(!predicate()){checkTime();await new Promise(resolve=>setTimeout(resolve,100));}
        checkTime();
      };
      pendingControl={id,action:'stop_and_pause',phase:'validating'};
      try{
        checkTime();checkExpiry();
        if(!expected||typeof expectedSession!=='string'||session()!==expectedSession||
            String(store.accInfo?.game_id||'')!==expected||store.accInfo?.accStatus!=='speeding')failure('game_changed');
        const account=findUserStore();
        if(!account||account.isLogin!==true||typeof account.toggleTimeStatus!=='function')failure('pause_unavailable');
        const accountKey=JSON.stringify([text(account.userInfo?.nn_id),text(account.userInfo?.nn_number)]);
        const sameAccount=()=>findUserStore()===account&&account.isLogin===true&&JSON.stringify([text(account.userInfo?.nn_id),text(account.userInfo?.nn_number)])===accountKey;
        if(!['pause','timeing'].includes(account.userTimeInfo?.timeStatus)||account.progressing)failure('billing_not_ready');
        pendingControl.phase='stopping';
        await bounded(()=>{checkTime();checkExpiry();unchanged();if(String(store.accInfo?.game_id||'')!==expected||store.accInfo?.accStatus!=='speeding')failure('game_changed');
          return store.stopAcc({game_id:expected,isConfirm:false,ipcReport:true,reason:'leigod_status_auto_stop'});},'stop');
        await wait(()=>{unchanged();return store.accInfo?.accStatus==='normal';},'stop');
        idle();stopped=true;checkTime();
        if(!sameAccount())failure('account_changed');
        if(account.userTimeInfo?.timeStatus==='pause')paused=true;
        else{
          if(account.progressing||account.userTimeInfo?.timeStatus!=='timeing')failure('billing_not_ready');
          idle();pendingControl.phase='pausing';
          await bounded(()=>{checkTime();idle();if(!sameAccount())failure('account_changed');
            return pauseAccount(account,'leigod_status_auto_stop',()=>{checkTime();idle();if(!sameAccount())failure('account_changed');});},'pause');
          await wait(()=>{idle();if(!sameAccount())failure('account_changed');return account.userTimeInfo?.timeStatus==='pause';},'pause');
          paused=true;
        }
        idle();checkTime();
        lastControlResult=result(true,null,{stopped,paused,completed_at_ms:Date.now()});
      }catch(error){
        const allowed=new Set(['observer_disposed','exit_requested','expired','timeout','game_changed','pause_unavailable','billing_not_ready','account_changed','stop_timeout','stop_failed','pause_timeout','pause_failed']);
        lastControlResult=result(false,allowed.has(error?.message)?error.message:'control_failed',{stopped,paused,completed_at_ms:Date.now()});
      }finally{pendingControl=null;}
      return lastControlResult;
    },
    async snapshot() {
      const currentSession=session();
      const info=store.accInfo;
      const status=String(info?.accStatus||'unknown');
      const state=status==='speeding'?'active':status==='normal'?'inactive':'unknown';
      const id=info?.game_id;
      let game=null;
      if(id) {
        let params=null;
        try { params=info.getAccParams?.(); } catch (_) {}
        const detail=params?.gameInfo;
        let name=detail?.title||params?.game_name||null;
        let configured=names(detail?.game_process);
        let metadataSource='live_acc_params';
        if(!name||!configured.length) {
          try {
            const cached=await Promise.race([localGame(id),new Promise(resolve=>setTimeout(()=>resolve(null),1500))]);
            if(cached) { name=name||cached.name; if(!configured.length)configured=cached.processes; metadataSource='live_params_and_local_game_list'; }
          } catch (_) {}
        }
        // Recheck after async DB reads so a stopped/switched game is not stale.
        if(store.accInfo?.game_id!==id||String(store.accInfo?.accStatus)!==status||session()!==currentSession)return monitor.snapshot();
        game={id:String(id),session:currentSession,name,configured_process_names:configured,metadata_source:metadataSource};
      }
      return {available:true,state,acc_status:status,game,last_action:lastAction,control_pending:pendingControl,control_result:lastControlResult,source:'renderer_pinia_acc_store',...userSnapshot()};
    },
    dispose() { disposed=true;unsubscribe(); userUnsubscribe?.(); delete window[key]; }
  };
  Object.defineProperty(window,key,{value:monitor,configurable:true});
  return true;
})()
