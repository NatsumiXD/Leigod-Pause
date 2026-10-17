'use strict';

// Elevated helper. The controller owns configuration/source and selects an
// existing PID. This helper never launches a target or uses stdout as transport.
const fs = require('node:fs');
const path = require('node:path');
let config;
let eventsFd;
let session;
let script;
let scriptLoaded = false;
let startup = Promise.resolve();
let pollTimer;
let stopTimer;
let stopping = false;
let detached = false;
let shutdown;
let exitCode = 0;
let exiting = false;
let exitPause;

function errorText(error) {
  return error instanceof Error ? error.message : String(error);
}
function emit(value) {
  if (eventsFd === undefined) return;
  fs.writeSync(eventsFd, JSON.stringify(value) + '\n', null, 'utf8');
}
function finish(code) {
  if (eventsFd !== undefined) {
    try { fs.closeSync(eventsFd); } catch (_) {}
    eventsFd = undefined;
  }
  process.exit(code);
}
async function bounded(operation, milliseconds, name) {
  let timer;
  try {
    return await Promise.race([
      Promise.resolve().then(operation),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(name + ' timed out')), milliseconds);
      }),
    ]);
  } catch (error) {
    emit({kind:'script_error', phase:name, error:errorText(error)});
    exitCode = Math.max(exitCode, 1);
    return undefined;
  } finally {
    clearTimeout(timer);
  }
}
function stop(code = 0) {
  exitCode = Math.max(exitCode, code);
  if (shutdown) return shutdown;
  stopping = true;
  try{fs.writeFileSync(config.transport.stopFile,'stop\n');}catch(_){}
  clearTimeout(pollTimer);
  clearInterval(stopTimer);
  const forceExit = setTimeout(() => {
    try { emit({kind:'error', error:'Graceful shutdown exceeded 8 seconds'}); }
    finally { finish(exitCode || 1); }
  }, 8000);
  shutdown = (async () => {
    await startup.catch(() => {});
    if (script && !detached) {
      if (scriptLoaded) {
        const result = await bounded(() => script.exports.stop(), 3500, 'agent_cleanup');
        if (!result || result.cleaned !== true) {
          emit({kind:'script_error', phase:'agent_cleanup',
            error:result && typeof result.error === 'string' ? result.error : 'Agent cleanup was not confirmed'});
          exitCode = Math.max(exitCode, 1);
        }
      }
      if (!detached) await bounded(() => script.unload(), 1500, 'script_unload');
    }
    if (session && !detached) await bounded(() => session.detach(), 1500, 'session_detach');
    clearTimeout(forceExit);
    finish(exitCode);
  })().catch(error => {
    try { emit({kind:'error', error:errorText(error)}); }
    finally { clearTimeout(forceExit); finish(1); }
  });
  return shutdown;
}

// stopFile means observer teardown/reconnect, not a user shutdown. Only the
// controller's disappearance or an explicit signal starts the billing action.
function pauseThenStop(code=0){
  if(exitPause)return exitPause;
  exiting=true;
  exitPause=(async()=>{
    const transport=config.transport;
    if(config.pauseOnExit===true&&transport.exitFile&&transport.exitResultFile&&transport.exitCommandId){
      const id=transport.exitCommandId;
      try{
        let result;
        try{result=JSON.parse(fs.readFileSync(transport.exitResultFile,'utf8'));}catch(_){}
        if(result?.id!==id){
          let existing;
          try{existing=JSON.parse(fs.readFileSync(transport.exitFile,'utf8'));}catch(_){}
          if(existing?.id!==id)fs.writeFileSync(transport.exitFile,JSON.stringify({id,action:'pause_before_exit',expires_at_ms:Date.now()+2000}));
          const deadline=Date.now()+3100;
          do{
            fs.writeFileSync(transport.heartbeatFile,'alive\n');
            try{result=JSON.parse(fs.readFileSync(transport.exitResultFile,'utf8'));}catch(_){}
            if(result?.id===id)break;
            await new Promise(resolve=>setTimeout(resolve,50));
          }while(!detached&&Date.now()<deadline);
        }
        emit({kind:'exit_pause',result:result?.id===id?{id,ok:result.ok===true,paused:result.paused===true,error:typeof result.error==='string'?result.error:null}:{id,ok:false,paused:false,error:'pause_timeout'}});
      }catch(_){emit({kind:'exit_pause',result:{id,ok:false,paused:false,error:'exit_transport_unavailable'}});}
    }
    return stop(code);
  })();
  return exitPause;
}

async function start() {
  const frida = require(config.fridaModule);
  const device = await frida.getLocalDevice();
  if (stopping) return;
  session = await device.attach(config.pid);
  session.detached.connect(reason => {
    if (!detached) emit({kind:'detached', reason});
    detached = true;
    if (!stopping) void stop(1);
  });
  if (stopping) return;
  fs.writeFileSync(config.transport.heartbeatFile,'alive\n');
  script = await session.createScript('globalThis.LEIGOD_NAPI_TRANSPORT='+JSON.stringify(config.transport)+';\n'+config.source);
  // Suppress log content: the owned agent's structured messages are the only
  // data transport. Frida's default logger would otherwise write to stdout.
  script.logHandler = level => emit({kind:'script_error',
    error:'Script console log suppressed', level});
  script.message.connect(message => {
    if (message.type === 'send') {
      // The script is controller-owned and emits sanitized data only. Do not
      // forward the separate binary argument or any process-memory payload.
      emit({kind:'hook', payload:message.payload});
    } else if (message.type === 'error') {
      emit({kind:'script_error', error:{description:message.description,
        stack:message.stack, fileName:message.fileName,
        lineNumber:message.lineNumber, columnNumber:message.columnNumber}});
    }
  });
  if (stopping) return;
  await script.load();
  scriptLoaded = true;
  if (stopping) return;
  emit({kind:'attached', pid:config.pid});
  async function poll() {
    if (stopping || detached) return;
    try {
      fs.writeFileSync(config.transport.heartbeatFile,'alive\n');
      let snapshot;
      try{
        snapshot=JSON.parse(fs.readFileSync(config.transport.snapshotFile,'utf8'));
        if(Date.now()-snapshot.observed_at_ms>15000)snapshot=null;
      }catch(_){}
      if(!snapshot)snapshot = await script.exports.snapshot(); // Agent's cached data only.
      if (!stopping) emit({kind:'snapshot', snapshot, pid:config.pid});
    } catch (error) {
      if (!stopping) {
        emit({kind:'script_error', error:errorText(error)});
        void stop(1);
      }
      return;
    }
    if (!stopping) pollTimer = setTimeout(poll, config.intervalMs);
  }
  pollTimer = setTimeout(poll, 0); // Schedule after completion: no overlapping RPC.
}

try {
  if (!process.argv[2] || !path.isAbsolute(process.argv[2])) {
    throw new Error('Usage: node napi-bridge.cjs <absolute-config.json>');
  }
  config = JSON.parse(fs.readFileSync(process.argv[2], 'utf8').replace(/^\uFEFF/, ''));
  if (!config || !config.transport ||
      typeof config.transport.eventsFile !== 'string' || !path.isAbsolute(config.transport.eventsFile) ||
      typeof config.transport.stopFile !== 'string' || !path.isAbsolute(config.transport.stopFile)) {
    throw new Error('transport.eventsFile and transport.stopFile must be absolute paths');
  }
  eventsFd = fs.openSync(config.transport.eventsFile, 'a');
  if (typeof config.fridaModule !== 'string' || !config.fridaModule ||
      !Number.isInteger(config.pid) || config.pid <= 0 ||
      typeof config.source !== 'string' || !config.source ||
      !Number.isFinite(config.intervalMs) || config.intervalMs < 100) {
    throw new Error('Invalid bridge configuration');
  }
  process.on('SIGINT', () => { void pauseThenStop(); });
  process.on('SIGTERM', () => { void pauseThenStop(); });
  stopTimer = setInterval(() => {
    if(!stopping)try{fs.writeFileSync(config.transport.heartbeatFile,'alive\n');}catch(_){}
    if (!stopping && !exiting && fs.existsSync(config.transport.stopFile)) void stop();
    if (!stopping && !exiting && Number.isInteger(config.controllerPid) && config.controllerPid > 0) {
      try { process.kill(config.controllerPid,0); } catch (error) { if(error.code==='ESRCH')void pauseThenStop(); }
    }
  }, 200);
  if (fs.existsSync(config.transport.stopFile)) {
    void stop();
  } else {
    startup = start();
    startup.catch(error => {
      emit({kind:'error', error:errorText(error)});
      void stop(1);
    });
  }
} catch (error) {
  try { emit({kind:'error', error:errorText(error)}); }
  finally { process.stderr.write(errorText(error) + '\n'); finish(1); }
}
