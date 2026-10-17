'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');

function fixture() {
  const hooks = new Map(), memory = new Map();
  class Pointer {
    constructor(n) { this.n = n; }
    add(n) { return new Pointer(this.n + n); }
    readU8() { return memory.get(this.n) || 0; }
    readU16() { return memory.get(this.n) || 0; }
    readU32() { return memory.get(this.n) || 0; }
    readS32() { return memory.get(this.n) || 0; }
    readPointer() { return new Pointer(memory.get(this.n) || 0); }
    isNull() { return this.n === 0; }
    equals(p) { return this.n === p.n; }
    toUInt32() { return this.n >>> 0; }
    toInt32() { return this.n | 0; }
    readByteArray(length) { return Uint8Array.from({length}, (_, i) => this.add(i).readU8()).buffer; }
  }
  const base = 0x10000000, runtime = 0x20000000;
  memory.set(base + 0x9802a8, runtime); memory.set(runtime + 12, 1);
  const module = {name:'GateProxyEx.dll',path:'C:/fixture/GateProxyEx.dll',base:new Pointer(base),getExportByName:name=>name};
  const sandbox = {MONITOR_CONFIG:{modulePath:module.path},rpc:{exports:{}},send:()=>{},
    Interceptor:{attach:(name,cb)=>{hooks.set(name,cb);return{detach:()=>hooks.delete(name)};}},
    Process:{arch:'ia32',id:99,attachModuleObserver:cb=>{
      sandbox.observer=cb;
      // Only the query export is read as a pointer; other exports are opaque keys.
      module.getExportByName=name=>name==='getUdpConnInfo'?new Pointer(0x3000):name;
      cb.onAdded(module); return {}; }} };
  vm.runInNewContext(fs.readFileSync(require('node:path').join(__dirname,'../hook/agent.js'),'utf8'),sandbox);
  const ptr=n=>new Pointer(n);
  function enter(name,args,threadId=1) { const context={threadId}; hooks.get(name).onEnter?.call(context,args.map(ptr)); return context; }
  function leave(name,context,result=0) { hooks.get(name).onLeave?.call(context,ptr(result)); }
  return {hooks,memory,runtime,base,module,sandbox,ptr,enter,leave,snapshot:()=>sandbox.rpc.exports.snapshot()};
}

test('active on attach preserves incomplete baseline; inactive clears connections',()=>{
  const f=fixture();
  assert.equal(f.snapshot().state,'active'); assert.equal(f.snapshot().processes_complete,false);
  const info=0x4000; f.memory.set(info+4,4321); f.memory.set(info+39,2); f.memory.set(info+43,8);
  const call=f.enter('tcpConnectRequest',[5,0,info,0]);
  f.memory.set(info+43,127); f.leave('tcpConnectRequest',call);
  assert.equal(f.snapshot().connections.length,1);
  f.memory.set(f.runtime+12,0);
  assert.equal(f.snapshot().state,'inactive'); assert.equal(f.snapshot().connections.length,0);
});
test('direct TCP is excluded, redirected TCP removed on close',()=>{
  const f=fixture(), info=0x4000;
  f.memory.set(info+4,4321); f.memory.set(info+39,2); f.memory.set(info+43,8);
  let call=f.enter('tcpConnectRequest',[5,0,info,0]); f.leave('tcpConnectRequest',call);
  assert.equal(f.snapshot().connections.length,0);
  call=f.enter('tcpConnectRequest',[5,0,info,0]); f.memory.set(info+43,127); f.leave('tcpConnectRequest',call);
  assert.equal(f.snapshot().connections[0].pid,4321);
  f.enter('tcpClosed',[5,0,info,0]); assert.equal(f.snapshot().connections.length,0);
});
test('UDP requires both successful PID query and consumption return; close removes endpoint',()=>{
  const f=fixture(), output=0x5000, query=[...f.hooks.keys()].find(x=>typeof x==='object');
  f.memory.set(output,4321);
  function udp(result,threadId=1,queryResult=0) {
    const call=f.enter('udpSend',[9,0,0,0,0,0,0],threadId);
    const q=f.enter(query,[9,0,output],threadId); f.leave(query,q,queryResult);
    f.leave('udpSend',call,result);
  }
  udp(0); assert.equal(f.snapshot().connections.length,0);
  udp(1,1,1); assert.equal(f.snapshot().connections.length,0);
  udp(1); assert.equal(f.snapshot().connections[0].protocol,'udp');
  f.enter('udpClosed',[9,0,0,0]); assert.equal(f.snapshot().connections.length,0);
});
test('transition and unreadable memory report unknown rather than stale active',()=>{
  const f=fixture();
  const call=f.enter('stop_proxy',[]);
  assert.equal(f.snapshot().state,'unknown'); assert.equal(f.snapshot().connections.length,0);
  f.memory.set(f.runtime+12,0); f.leave('stop_proxy',call);
  assert.equal(f.snapshot().state,'inactive'); assert.equal(f.snapshot().processes_complete,true);
  f.memory.set(f.base+0x9802a8,0); assert.equal(f.snapshot().state,'unknown');
});
