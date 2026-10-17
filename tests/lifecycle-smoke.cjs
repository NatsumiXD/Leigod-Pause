'use strict';
// Only synthetic sleep processes under a unique test directory are targeted.
const {spawn}=require('node:child_process'),fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');
const root=path.resolve('target/lifecycle-'+Date.now()),exe=path.resolve('target/launcher/debug/leigod-status.exe'),fixture=path.resolve('target/refresh-fixture/leigod.exe');
const owned=new Set();
const delay=ms=>new Promise(r=>setTimeout(r,ms));
function launch(file,args=[],env=process.env){const p=spawn(file,args,{env,windowsHide:true,stdio:['ignore','pipe','pipe']});owned.add(p);return p;}
function exited(p){return p.exitCode!==null||p.signalCode!==null;}
async function wait(fn,label){const end=Date.now()+8000;while(!fn()){if(Date.now()>end)throw Error('Timeout: '+label);await delay(40);}}
async function scenario(mode){
 const install=path.join(root,mode),other=path.join(root,mode+'-other');fs.mkdirSync(path.join(install,'resources'),{recursive:true});fs.mkdirSync(other,{recursive:true});
 for(const name of ['leigod.exe','game.exe'])fs.copyFileSync(fixture,path.join(install,name));fs.copyFileSync(fixture,path.join(other,'leigod.exe'));
 const game=launch(path.join(install,'game.exe')),unrelated=launch(path.join(other,'leigod.exe'));
 const endpoint={request_path:path.join(install,'exit-request.json'),result_path:path.join(install,'exit-result.json'),heartbeat_path:path.join(install,'heartbeat'),id:'exit-'+mode};
 const endpointFile=path.join(install,'endpoint.json');fs.writeFileSync(endpointFile,JSON.stringify(endpoint));
 const controller=launch(exe,['--test-binding',path.join(install,'leigod.exe'),...(['normal','pause-timeout'].includes(mode)?['quit']:[])],{...process.env,LEIGOD_EXIT_TEST_ENDPOINT:endpointFile});
 let output='',error='';controller.stdout.on('data',s=>output+=s);controller.stderr.on('data',s=>error+=s);
 await wait(()=>/fixture_ready (\d+)/.test(output)||exited(controller),'binding ready');assert.match(output,/fixture_ready (\d+)/,error);
 const clientPid=Number(output.match(/fixture_ready (\d+)/)[1]);await delay(350);
 let pauseRequests=0,clientAliveAtPause=false,pauseFinished=false;
 const pauseTimer=setInterval(()=>{
   if(pauseRequests||!fs.existsSync(endpoint.request_path))return;
   let command;try{command=JSON.parse(fs.readFileSync(endpoint.request_path,'utf8'));}catch(_){return;}
   assert.equal(command.action,'pause_before_exit');pauseRequests++;
   try{process.kill(clientPid,0);clientAliveAtPause=true;}catch(_){}
   setTimeout(()=>{try{if(mode!=='client-killed')process.kill(clientPid,0);if(mode==='pause-timeout')return;fs.writeFileSync(endpoint.result_path,JSON.stringify({id:command.id,ok:mode!=='client-killed',paused:mode!=='client-killed',error:mode==='client-killed'?'renderer_unavailable':null}));pauseFinished=true;}catch(error){console.error(error);process.exitCode=1;}},300);
 },25);
 if(mode==='controller-killed')controller.kill();if(mode==='client-killed')process.kill(clientPid);
 try{
  await wait(()=>exited(controller),'controller exit');
  await wait(()=>{try{process.kill(clientPid,0);return false;}catch(_){return true;}},'client exit');
  assert.equal(pauseRequests,1,'exit pause must be requested once');assert.equal(pauseFinished,mode!=='pause-timeout','pause result must precede process cleanup, unless the bounded wait expires');
  assert.equal(clientAliveAtPause,mode!=='client-killed','running client must survive until pause request');
  const logDirectory=path.join(path.dirname(exe),'logs');
  const audits=fs.readdirSync(logDirectory).filter(name=>name.startsWith('exit-'+controller.pid+'-')&&name.endsWith('.json'));
  assert.equal(audits.length,1);const audit=JSON.parse(fs.readFileSync(path.join(logDirectory,audits[0]),'utf8'));
  assert.equal(audit.paused,!['client-killed','pause-timeout'].includes(mode));
  if(mode==='pause-timeout')assert.equal(audit.error,'pause_not_confirmed_before_exit');
 }finally{clearInterval(pauseTimer);}
 assert.equal(exited(game),false,'game must survive');assert.equal(exited(unrelated),false,'other install must survive');game.kill();unrelated.kill();
 console.log(mode+': pause attempted before cleanup; paired exit passed; game and other installation preserved');
}
(async()=>{try{for(const mode of ['normal','controller-killed','client-killed','pause-timeout'])await scenario(mode);}finally{for(const p of owned)if(!exited(p))p.kill();}})().catch(e=>{console.error(e);process.exitCode=1;});
