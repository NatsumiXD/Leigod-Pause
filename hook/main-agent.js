// Install through Node-API in Electron main; no original application files change.
(() => {
  const config=globalThis.LEIGOD_MAIN_CONFIG;
  if(!config||typeof config.key!=='string')throw new Error('Missing main observer configuration');
  const transport=globalThis.LEIGOD_MAIN_TRANSPORT;
  const prior=globalThis[config.key];
  if(prior?.control_version===2&&prior.transport_key===(transport?.snapshotFile||null))return true;
  prior?.dispose?.();
  const {ipcMain,BrowserWindow,app}=process.mainModule.require('electron');
  const timers=process.mainModule.require('node:timers');
  const fs=transport?process.mainModule.require('node:fs'):null;
  const interval=Math.max(250,Number(config.intervalMs)||1000);
  const leaseMs=Math.max(15000,interval*5);
  let leaseUntil=Date.now()+leaseMs,disposed=false,polling=false;
  let latest={available:false,state:'unknown',error:'Waiting for renderer snapshot'};
  let backendState='unknown',backendGame=null,lastIpcAction=null;
  let controlPage=null,controlBusy=false,controlResult=null,commandTimer=null;
  let exiting=false,exitPromise=null,exitResult=null,quittingAllowed=false,quitPromise=null;
  const commandIds=new Set();
  const wrappers=[];
  const pages=new Set();
  const isHandlerMap=value=>value&&typeof value.get==='function'&&typeof value.set==='function'&&typeof value.has==='function';
  let handlerMap=ipcMain._invokeHandlers;
  if(!isHandlerMap(handlerMap))handlerMap=Object.values(ipcMain).find(v=>isHandlerMap(v)&&v.has('start-acc')&&v.has('stop-acc'));
  function codeOf(value){return value?.result?.code??value?.code??value?.data?.code;}
  function identity(data){
    if(!data||typeof data!=='object')return null;
    const id=data.game_id??data.id;
    return id===undefined?null:{id:String(id),name:typeof data.game_name==='string'?data.game_name:null,configured_process_names:[],metadata_source:'start_acc_ipc'};
  }
  if(isHandlerMap(handlerMap)){
    for(const channel of ['start-acc','stop-acc']){
      const original=handlerMap.get(channel);if(typeof original!=='function')continue;
      const wrapper=function(event,...args){
        const action={name:channel,phase:'requested',time_ms:Date.now()};lastIpcAction=action;
        if(channel==='start-acc')backendGame=identity(args[0]);
        const returned=value=>{
          try{action.phase='returned';const code=codeOf(value);action.code=typeof code==='number'||typeof code==='string'?code:null;
            if(Number(code)===200)backendState=channel==='start-acc'?'active':'inactive';
            else if(value?.error||code!==undefined)backendState='unknown';
          }catch(_){}
        };
        const failed=()=>{action.phase='failed';backendState='unknown';};
        try{
          const result=original.apply(this,[event,...args]);
          if(result&&typeof result.then==='function')result.then(returned,failed);else returned(result);
          return result;
        }catch(error){failed();throw error;}
      };
      handlerMap.set(channel,wrapper);wrappers.push({channel,original,wrapper});
    }
  }
  const resource=config.resourcePath.replace(/\\/g,'/').toLowerCase();
  function trusted(page){
    if(!page||page.isDestroyed())return false;
    try{return decodeURI(page.getURL()).replace(/\\/g,'/').toLowerCase().startsWith('file:///'+resource+'/renderer.asar/');}catch(_){return false;}
  }
  function publishControl(value){
    controlResult=value;
    try{fs.writeFileSync(transport.resultFile,JSON.stringify(value));}catch(_){}
  }
  function publishExit(value){
    exitResult=value;
    if(transport?.exitResultFile)try{fs.writeFileSync(transport.exitResultFile,JSON.stringify(value));}catch(_){}
    return value;
  }
  async function requestExit(command){
    if(exitPromise)return exitPromise;
    const id=typeof command?.id==='string'?command.id:null;
    const complete=value=>exitResult?.id===id?exitResult:publishExit(value);
    const fail=error=>complete({id,ok:false,paused:false,error,completed_at_ms:Date.now()});
    if(!id||id.length>128||command?.action!=='pause_before_exit'||!Number.isFinite(command.expires_at_ms))return fail('invalid_command');
    if(command.expires_at_ms<=Date.now())return fail('expired');
    exiting=true;
    exitPromise=(async()=>{
      let timeout;
      try{
        const work=(async()=>{
          if(!trusted(controlPage)){
            const page=BrowserWindow.getAllWindows().filter(w=>!w.isDestroyed()).map(w=>w.webContents).find(trusted);
            if(!page)return fail('renderer_unavailable');
            if(!await page.executeJavaScript(config.uiSource))return fail('renderer_unavailable');
            pages.add(page);controlPage=page;
          }
          const outcome=await controlPage.executeJavaScript('window['+JSON.stringify(config.uiKey)+']?.control('+JSON.stringify(command)+')');
          if(!outcome||outcome.id!==id||typeof outcome.ok!=='boolean')return fail('control_unavailable');
          const errors=new Set(['invalid_command','observer_disposed','expired','command_capacity','pause_unavailable','account_changed','billing_not_ready','pause_timeout','pause_failed']);
          return complete({id,ok:outcome.ok&&outcome.paused===true,paused:outcome.paused===true,
            error:outcome.ok&&outcome.paused===true?null:errors.has(outcome.error)?outcome.error:'pause_failed',completed_at_ms:Date.now()});
        })();
        return await Promise.race([work,new Promise(resolve=>{timeout=timers.setTimeout(()=>resolve(fail('pause_timeout')),2900);})]);
      }catch(_){return fail('control_unavailable');}
      finally{timers.clearTimeout(timeout);}
    })();
    return exitPromise;
  }
  function beforeQuit(event){
    if(quittingAllowed||disposed||config.pauseOnExit!==true)return;
    event.preventDefault();
    if(quitPromise)return;
    quitPromise=requestExit({id:transport?.exitCommandId||'app-exit-'+Date.now(),action:'pause_before_exit',expires_at_ms:Date.now()+2000})
      .finally(()=>{quittingAllowed=true;app.quit();});
  }
  async function pollCommand(){
    if(disposed)return;
    try{if(fs.existsSync(transport.stopFile)||Date.now()-fs.statSync(transport.heartbeatFile).mtimeMs>leaseMs){observer.dispose();return;}}catch(_){observer.dispose();return;}
    // Exit has its own lane so a slow automatic stop cannot block pausing.
    if(config.pauseOnExit===true&&transport?.exitFile&&transport?.exitResultFile&&!exitPromise){
      try{
        if(fs.existsSync(transport.exitFile)){
          const text=fs.readFileSync(transport.exitFile,'utf8');
          if(text.length<=4096){const command=JSON.parse(text);if(command.id===transport.exitCommandId){await requestExit(command);return;}}
        }
      }catch(_){}
    }
    if(exiting||controlBusy||!transport?.commandFile||!transport?.resultFile)return;
    let raw;
    try{if(!fs.existsSync(transport.commandFile))return;const text=fs.readFileSync(transport.commandFile,'utf8');if(text.length>4096)return;raw=JSON.parse(text);}catch(_){return;}
    const id=typeof raw?.id==='string'&&raw.id.length>0&&raw.id.length<=128?raw.id:null;
    if(!id||commandIds.has(id))return;
    if(commandIds.size>=4096){publishControl({id,ok:false,error:'command_capacity'});return;}
    commandIds.add(id);
    const command={id,action:raw.action,expected_game_id:raw.expected_game_id,
      expected_session:raw.expected_session,expires_at_ms:raw.expires_at_ms};
    const fail=error=>publishControl({id,ok:false,error,completed_at_ms:Date.now()});
    if(command.action!=='stop_and_pause'||typeof command.expected_game_id!=='string'||!command.expected_game_id||command.expected_game_id.length>64||
       typeof command.expected_session!=='string'||!command.expected_session||command.expected_session.length>256||!Number.isFinite(command.expires_at_ms))return fail('invalid_command');
    if(command.expires_at_ms<=Date.now())return fail('expired');
    if(!trusted(controlPage))return fail('renderer_unavailable');
    controlBusy=true;let timeout;
    try{
      const expression='window['+JSON.stringify(config.uiKey)+']?.control('+JSON.stringify(command)+')';
      const outcome=await Promise.race([controlPage.executeJavaScript(expression),new Promise((_,reject)=>{
        timeout=timers.setTimeout(()=>reject(new Error('timeout')),12000);
      })]);
      const errors=new Set(['invalid_command','replay','command_capacity','control_busy','exit_requested','observer_disposed','expired','timeout','game_changed','pause_unavailable','billing_not_ready','account_changed','stop_timeout','stop_failed','pause_timeout','pause_failed','control_failed']);
      if(!outcome||outcome.id!==id||typeof outcome.ok!=='boolean')return fail('control_unavailable');
      publishControl({id,ok:outcome.ok,error:outcome.ok?null:errors.has(outcome.error)?outcome.error:'control_failed',
        stopped:outcome.stopped===true,paused:outcome.paused===true,completed_at_ms:Date.now()});
    }catch(error){fail(error?.message==='timeout'?'timeout':'control_unavailable');}
    finally{timers.clearTimeout(timeout);controlBusy=false;}
  }
  async function poll(){
    if(disposed)return;
    if(transport){
      try{if(fs.existsSync(transport.stopFile)||Date.now()-fs.statSync(transport.heartbeatFile).mtimeMs>leaseMs){observer.dispose();return;}}
      catch(_){observer.dispose();return;}
    }else if(Date.now()>leaseUntil){observer.dispose();return;}
    if(polling)return;
    polling=true;
    try{
      const windows=BrowserWindow.getAllWindows().filter(w=>!w.isDestroyed()).sort((a,b)=>{
        const x=a.getSize(),y=b.getSize();return y[0]*y[1]-x[0]*x[1];
      });
      let found=null;
      for(const window of windows){
        const page=window.webContents;
        if(page.isDestroyed())continue;
        if(!trusted(page))continue;
        pages.add(page);
        if(!await page.executeJavaScript(config.uiSource))continue;
        if(disposed)return;
        const value=await page.executeJavaScript('window['+JSON.stringify(config.uiKey)+'].snapshot()');
        if(value?.available){found=value;controlPage=page;break;}
      }
      latest=found||{available:false,state:'unknown',error:'Acceleration renderer/store not ready'};
    }catch(error){latest={available:false,state:'unknown',error:String(error.message||error)};}
    finally{
      polling=false;
      if(transport&&!disposed){try{fs.writeFileSync(transport.snapshotFile,JSON.stringify({transport:'napi',installed:true,observed_at_ms:Date.now(),snapshot:observer.snapshot()}));}catch(_){}}
    }
  }
  const observer={
    control_version:2,transport_key:transport?.snapshotFile||null,
    snapshot(){
      leaseUntil=Date.now()+leaseMs;
      const result={...latest,transport:'napi',ipc_hook_available:wrappers.length===2,last_ipc_action:lastIpcAction,backend_state:backendState,
        control_pending:exiting?{action:'pause_before_exit'}:controlBusy?{action:'stop_and_pause'}:latest.control_pending||null,
        control_result:controlResult||latest.control_result||null,exit_result:exitResult};
      if(['active','inactive'].includes(backendState)){
        if(latest.available&&['active','inactive'].includes(latest.state)&&latest.state!==backendState){result.state='unknown';result.error='Renderer and completed IPC state disagree';}
        else if(!latest.available){result.available=true;result.state=backendState;result.error=null;result.game=backendState==='active'?backendGame:null;}
      }
      return result;
    },
    dispose(){
      if(disposed)return;disposed=true;timers.clearInterval(timer);if(commandTimer)timers.clearInterval(commandTimer);
      app?.removeListener?.('before-quit',beforeQuit);
      for(const {channel,original,wrapper} of wrappers)if(handlerMap.get(channel)===wrapper)handlerMap.set(channel,original);
      for(const page of pages)if(!page.isDestroyed())page.executeJavaScript('window['+JSON.stringify(config.uiKey)+']?.dispose()').catch(()=>{});
      delete globalThis[config.key];
    }
  };
  Object.defineProperty(globalThis,config.key,{value:observer,configurable:true});
  const timer=timers.setInterval(()=>{void poll();},interval);timer.unref?.();
  if(transport?.commandFile&&transport?.resultFile||config.pauseOnExit===true&&transport?.exitFile){commandTimer=timers.setInterval(()=>{void pollCommand();},150);commandTimer.unref?.();}
  if(config.pauseOnExit===true)app?.on?.('before-quit',beforeQuit);
  void poll();return true;
})()
