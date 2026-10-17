'use strict';
// Usage: node tests/bridge-smoke.cjs FRIDA_MODULE GATE_DLL FIXTURE_EXE
const {spawn} = require('node:child_process');
const readline=require('node:readline');
const assert=require('node:assert/strict');
const path=require('node:path');
const [fridaModule,dll,fixtureExe]=process.argv.slice(2);
const fullAgent=process.argv.includes('--agent');
const host=spawn(fixtureExe,[],{windowsHide:true,stdio:'ignore'});
const bridge=spawn(process.execPath,[path.join(__dirname,'../hook/bridge.cjs')],{windowsHide:true,stdio:['pipe','pipe','inherit']});
const timer=setTimeout(()=>finish(new Error('smoke test timed out')),15000);
let gotSnapshot=false, finished=false;
function finish(error) {
  if(finished)return;finished=true;clearTimeout(timer);host.kill();bridge.kill();
  if(error){console.error(error);process.exitCode=1;} else console.log('Frida attach/RPC/detach smoke test passed');
}
host.on('error',finish);bridge.on('error',finish);
const lines=readline.createInterface({input:bridge.stdout});
lines.on('line',line=>{
  const event=JSON.parse(line);
  if(event.kind==='error'||event.kind==='script_error') return finish(new Error(JSON.stringify(event)));
  if(event.kind==='snapshot') {
    try {
      if(fullAgent) { assert.equal(event.snapshot.sdk_pid,host.pid); assert.equal(event.snapshot.state,'unknown'); }
      else { assert.equal(event.snapshot.pid,host.pid); assert.equal(event.snapshot.mode,'fixture'); }
    }
    catch(e){return finish(e);}
    gotSnapshot=true;bridge.stdin.end(JSON.stringify({command:'stop'})+'\n');
  }
});
bridge.on('exit',code=>finish(code===0&&gotSnapshot?null:new Error('Bridge exited '+code)));
bridge.stdin.write(JSON.stringify({fridaModule:path.resolve(fridaModule),pid:host.pid,
  source:fullAgent?'const MONITOR_CONFIG='+JSON.stringify({modulePath:path.resolve(dll)})+';\n'+require('node:fs').readFileSync(path.join(__dirname,'../hook/agent.js'),'utf8'):
    'rpc.exports={snapshot(){return {pid:Process.id,mode:"fixture"};}};',intervalMs:100,
  verification:{path:path.resolve(dll),sha256:'aca4e862d4f26073c99dfbe77bb750effbedfe805b6b5bac34b284220d750153'}})+'\n');
