'use strict';
// Owned fixture only: real Rust controller + process inventory + local CDP.
// Usage: node tests/refresh-smoke.cjs PATH_TO_WS_MODULE
const {WebSocketServer}=require(require('node:path').resolve(process.argv[2]));
const http=require('node:http'),vm=require('node:vm'),path=require('node:path'),assert=require('node:assert/strict');
const {spawn}=require('node:child_process'),readline=require('node:readline'),{pathToFileURL}=require('node:url');
const root=path.resolve(__dirname,'..'),installation=path.join(root,'target/refresh-fixture');
let controller,host,finished=false,active=[],reads=0;
let title='Fixture game',processName='FixtureNeverRuns.exe';
const info={game_id:42,accStatus:'speeding',getAccParams(){reads++;return{gameInfo:{title,game_process:processName}};}};
const store={accInfo:info,startAcc(){},stopAcc(){},$onAction(){return()=>{};}};
const context=vm.createContext({Map,window:{},setTimeout,document:{getElementById:()=>({__vue_app__:{config:{globalProperties:{$pinia:{_s:new Map([['renamed-acc',store]])}}}}})}});
const server=http.createServer((_req,res)=>{
  res.setHeader('Content-Type','application/json');res.end(JSON.stringify([{type:'page',
    url:pathToFileURL(path.join(installation,'resources/renderer.asar/index.html')).href,
    webSocketDebuggerUrl:'ws://127.0.0.1:'+server.address().port+'/devtools/page/fixture'}]));
});
const ws=new WebSocketServer({server});
ws.on('connection',socket=>socket.on('message',async bytes=>{
  const request=JSON.parse(String(bytes));
  try{assert.equal(request.method,'Runtime.evaluate');const value=await vm.runInContext(request.params.expression,context);
    socket.send(JSON.stringify({id:request.id,result:{result:{type:typeof value,value}}}));
  }catch(error){socket.send(JSON.stringify({id:request.id,error:{message:error.message}}));}
}));
const timer=setTimeout(()=>finish(new Error('Periodic refresh fixture timed out')),25000);
function finish(error){
  if(finished)return;finished=true;clearTimeout(timer);controller?.kill();host?.kill();
  // Closing Rust's stdin pipe causes its owned UI bridge to stop itself.
  setTimeout(()=>{for(const client of ws.clients)client.terminate();ws.close();server.close();
    if(error){console.error(error);process.exitCode=1;}
    else console.log('5-second repeated logs, fresh renderer reads and absent game process names passed');
  },1500);
}
server.listen(0,'127.0.0.1',()=>{
  host=spawn(path.join(installation,'leigod.exe'),[],{windowsHide:true,stdio:'ignore'});host.on('error',finish);
  host.once('spawn',()=>{
    controller=spawn(process.argv[3]?path.resolve(process.argv[3]):path.join(root,'target/debug/leigod-status.exe'),['--no-launch','--cdp','--port',String(server.address().port),'--leigod-exe',installation,'--json'],
      {cwd:root,windowsHide:true,stdio:['ignore','pipe','pipe']});
    controller.on('error',finish);controller.on('exit',code=>{if(!finished)finish(new Error('Unexpected controller exit '+code));});
    readline.createInterface({input:controller.stdout}).on('line',line=>{
      try{
        const value=JSON.parse(line);if(value.state!=='active')return;
        active.push({at:Date.now(),value});
        assert.deepEqual(value.processes,[]);assert.deepEqual(value.not_running_process_names,[processName]);
        if(active.length===1){assert.equal(value.game.name,'Fixture game');}
        if(active.length===2){
          const payload=v=>{const copy={...v};delete copy.sampled_at_ms;delete copy.sample_number;return copy;};
          assert.deepEqual(payload(active[0].value),payload(value),'Unchanged snapshots must still be printed');
          assert.ok(value.sample_number>active[0].value.sample_number);assert.ok(value.sampled_at_ms>active[0].value.sampled_at_ms);
          const gap=active[1].at-active[0].at;assert.ok(gap>=4000&&gap<=8000,'Expected about 5 seconds, got '+gap);
          assert.ok(reads>=2,'Renderer must be read again');title='Refreshed game';processName='FixtureFreshProcess.exe';
        }
        if(active.length===3){assert.equal(value.game.name,'Refreshed game');assert.ok(reads>=3);finish();}
      }catch(error){finish(error);}
    });
  });
});
