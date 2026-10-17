'use strict';
// Local CDP fixture. Usage: node tests/ui-bridge-smoke.cjs PATH_TO_WS_MODULE
const {WebSocketServer}=require(require('node:path').resolve(process.argv[2]));
const http=require('node:http'),vm=require('node:vm'),fs=require('node:fs'),path=require('node:path');
const {spawn}=require('node:child_process'),readline=require('node:readline'),assert=require('node:assert/strict');
const agent=fs.readFileSync(path.join(__dirname,'../hook/ui-agent.js'),'utf8');
const key='__leigodReadOnlyMonitor_fixture';
const portFile=path.join(__dirname,'../target/ui-fixture-DevToolsActivePort.txt');
let actionCallback,disposed=false,phase=0,child,finished=false;
const info={game_id:'',accStatus:'normal',getAccParams:null};
const store={accInfo:info,startAcc(){},stopAcc(){},$onAction(cb){actionCallback=cb;return()=>{disposed=true;};}};
const context=vm.createContext({Map,window:{},setTimeout,document:{getElementById:()=>({__vue_app__:{config:{globalProperties:{$pinia:{_s:new Map([['renamedAccStore',store]])}}}}})}});
const server=http.createServer((req,res)=>{
  res.setHeader('Content-Type','application/json');
  res.end(JSON.stringify([{type:'page',url:'file:///C:/Fixture/resources/renderer.asar/index.html',webSocketDebuggerUrl:'ws://127.0.0.1:'+server.address().port+'/devtools/page/test'}]));
});
const ws=new WebSocketServer({server});
ws.on('connection',socket=>socket.on('message',async bytes=>{
  const request=JSON.parse(String(bytes));
  try{
    assert.equal(request.method,'Runtime.evaluate');
    const value=await vm.runInContext(request.params.expression,context);
    socket.send(JSON.stringify({id:request.id,result:{result:{type:typeof value,value}}}));
  }catch(error){socket.send(JSON.stringify({id:request.id,error:{message:error.message}}));}
}));
const timer=setTimeout(()=>finish(new Error('CDP fixture timed out')),12000);
function action(name){actionCallback({name,after:fn=>fn(),onError:()=>{}});}
function finish(error){
  if(finished)return;finished=true;clearTimeout(timer);child?.kill();
  for(const client of ws.clients)client.terminate();ws.close();server.close();
  if(fs.existsSync(portFile))fs.unlinkSync(portFile);
  if(error){console.error(error);process.exitCode=1;}else console.log('CDP port discovery / inject / start / cancel-stop / stop / dispose passed');
}
server.listen(0,'127.0.0.1',()=>{
  fs.writeFileSync(portFile,String(server.address().port)+'\n/devtools/browser/fixture\n');
  child=spawn(process.execPath,[path.join(__dirname,'../hook/ui-bridge.cjs')],{windowsHide:true,stdio:['pipe','pipe','inherit']});
  child.on('error',finish);child.on('exit',code=>finish(code===0&&phase===4&&disposed?null:new Error('Unexpected CDP bridge exit '+code+' phase '+phase+' disposed '+disposed)));
  readline.createInterface({input:child.stdout}).on('line',line=>{
    try{
      const event=JSON.parse(line);assert.equal(event.kind,'snapshot');assert.equal(event.snapshot.available,true,JSON.stringify(event));
      const s=event.snapshot;
      if(phase===0){assert.equal(s.state,'inactive');phase=1;action('startAcc');Object.assign(info,{game_id:42,accStatus:'speeding',getAccParams:()=>({gameInfo:{title:'Fixture game',game_process:'Game.exe'}})});}
      else if(phase===1){assert.equal(s.state,'active');assert.equal(s.game.name,'Fixture game');assert.equal(s.last_action.name,'startAcc');phase=2;action('stopAcc');}
      else if(phase===2){assert.equal(s.state,'active');assert.equal(s.last_action.name,'stopAcc');phase=3;Object.assign(info,{game_id:'',accStatus:'normal',getAccParams:null});}
      else if(phase===3){assert.equal(s.state,'inactive');assert.equal(s.game,null);phase=4;child.stdin.end(JSON.stringify({command:'stop'})+'\n');}
    }catch(e){finish(e);}
  });
  child.stdin.write(JSON.stringify({port:65530,activePortFile:portFile,key,resourcePath:'C:/Fixture/resources',source:agent.replace('__leigodReadOnlyMonitor_v1',key),intervalMs:100})+'\n');
});
