'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');

// Model the native boundary, including thread affinity and exception ownership.
// This validates agent invariants, not real Frida injection or Electron ABI.
function fixture(options = {}) {
  const hooks = new Map(), memory = new Map(), events = [], calls = {};
  let next = 1, insideCallback = false, pending = false, clock = 10000;
  let runFailure = false, nested = null;
  class Pointer {
    constructor(value) { this.value = value; }
    toString() { return '0x' + this.value.toString(16); }
    toInt32() { return this.value | 0; }
    isNull() { return this.value === 0; }
    readPointer() { return memory.get(this.value) || new Pointer(0); }
    readU8() { return memory.get(this.value) || 0; }
    readU32() { return memory.get(this.value) || 0; }
    readUtf8String() { return memory.get(this.value); }
  }
  const alloc = value => { const p = new Pointer(next++); memory.set(p.value, value); return p; };
  const write = (p, value) => memory.set(p.value, value);
  const target = vm.createContext({process:{type:'browser', versions:{electron:'fixture'},
    mainModule:{require: name => {
      assert.equal(name, 'node:worker_threads');
      return {isMainThread:options.mainThread !== false};
    }}}});
  const native = {
    napi_is_exception_pending(env, out) { write(out, pending ? 1 : 0); return 0; },
    napi_open_handle_scope(env, out) { write(out, alloc(0)); return 0; },
    napi_close_handle_scope() { return 0; },
    napi_get_and_clear_last_exception(env, out) { pending = false; write(out, alloc('own exception')); return 0; },
    napi_create_string_utf8(env, source, length, out) {
      assert.equal(length, Buffer.byteLength(source.readUtf8String(), 'utf8'));
      write(out, alloc(source.readUtf8String())); return 0;
    },
    napi_run_script(env, source, out) {
      if (nested) { const callback = nested; nested = null; callback(); }
      if (runFailure) { pending = true; return 10; }
      write(out, alloc(vm.runInContext(source.readUtf8String(), target))); return 0;
    },
    napi_get_value_string_utf8(env, value, buffer, capacity, outLength) {
      const text = value.readUtf8String();
      write(outLength, Buffer.byteLength(text, 'utf8'));
      if (!buffer.isNull()) write(buffer, text);
      return 0;
    },
  };
  const module = {getExportByName: name => {
    if (options.missing?.includes(name)) throw new Error('missing export');
    return name;
  }};
  const sandbox = {LEIGOD_NAPI_CONFIG:{installSource:'globalThis.fixtureInstalled=(globalThis.fixtureInstalled||0)+1;true',
    snapshotExpression:'({state:"active",processes:["测试Game.exe"]})',
    cleanupSource:'delete globalThis.fixtureInstalled', transport:{snapshotFile:'fixture.json'}},
    Date:{now:()=>clock}, rpc:{exports:{}}, send:event=>events.push(event),
    setTimeout:()=>0, NULL:new Pointer(0),
    Memory:{alloc:()=>alloc(0),allocUtf8String:alloc},
    Process:{platform:'windows',arch:'ia32',pointerSize:4,id:99,
      getModuleByName:()=>module, findModuleByAddress:p=>({name:p.caller})},
    NativeFunction:function(name) { return (...args) => {
      assert.equal(insideCallback, true, name + ' must run inside the genuine callback');
      calls[name] = (calls[name] || 0) + 1;
      return native[name](...args);
    }; },
    Interceptor:{attach:(name, callbacks)=>{
      hooks.set(name, callbacks); return {detach:()=>hooks.delete(name)};
    }},
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../hook/napi-agent.js'),'utf8'), sandbox);
  function invoke(name, {caller='leigod.exe', status=0, threadId=1} = {}) {
    const wasInside = insideCallback;
    insideCallback = true;
    try {
      const context = {threadId, returnAddress:{caller}};
      const hook = hooks.get(name);
      hook.onEnter.call(context, [new Pointer(0xff)]);
      hook.onLeave.call(context, new Pointer(status));
    } finally { insideCallback = wasInside; }
  }
  return {hooks, events, calls, target, invoke, rpc:sandbox.rpc.exports,
    setPending:value=>pending=value, failRun:()=>runFailure=true,
    reenter:callback=>nested=callback, tick:()=>clock+=1000};
}

test('async callback API from engine installs observer; cached RPC never calls native APIs', () => {
  const f = fixture();
  f.invoke('napi_make_callback');
  assert.equal(f.target.fixtureInstalled, 1);
  assert.equal(f.target.LEIGOD_MAIN_TRANSPORT.snapshotFile, 'fixture.json');
  assert.equal(f.rpc.snapshot().snapshot.processes[0], '测试Game.exe');
  assert.ok(f.events.some(e=>e.event==='ready' && e.entry==='napi_make_callback'));
  const count = f.calls.napi_run_script;
  f.rpc.snapshot(); f.rpc.snapshot();
  assert.equal(f.calls.napi_run_script, count);
});

test('failed callbacks and preexisting exceptions never execute JS or clear application exceptions', () => {
  const f = fixture();
  f.invoke('napi_call_function', {status:10});
  assert.equal(f.calls.napi_is_exception_pending, undefined);
  f.setPending(true);
  f.invoke('napi_call_function');
  assert.equal(f.calls.napi_run_script, undefined);
  assert.equal(f.calls.napi_get_and_clear_last_exception, undefined);
});

test('get_cb_info still requires addon origin and worker environments do not install', () => {
  const f = fixture({mainThread:false});
  f.invoke('napi_get_cb_info');
  assert.equal(f.calls.napi_run_script, undefined);
  f.invoke('napi_get_cb_info', {caller:'fixture.node'});
  assert.equal(f.calls.napi_run_script, 1);
  assert.equal(f.target.fixtureInstalled, undefined);
  assert.equal(f.rpc.snapshot().installed, false);
});

test('nested native callback cannot recursively enter probe', () => {
  const f = fixture();
  f.reenter(()=>f.invoke('napi_call_function'));
  f.invoke('napi_make_callback');
  assert.equal(f.calls.napi_run_script, 1);
  assert.equal(f.calls.napi_open_handle_scope, f.calls.napi_close_handle_scope);
});

test('probe-created native exception is cleared and handle scope always closes', () => {
  const f = fixture();
  f.failRun(); f.invoke('napi_call_function');
  assert.equal(f.calls.napi_get_and_clear_last_exception, 1);
  assert.equal(f.calls.napi_close_handle_scope, 1);
  assert.ok(f.events.some(e=>e.event==='napi_error'));
});

test('cleanup waits for genuine callback, then removes all dynamic entry hooks', async () => {
  const f = fixture({missing:['napi_make_callback']});
  f.invoke('napi_call_function');
  const stop = f.rpc.stop();
  assert.equal(f.target.fixtureInstalled, 1);
  f.tick(); f.invoke('napi_call_function');
  assert.equal((await stop).cleaned, true);
  assert.equal(f.target.fixtureInstalled, undefined);
  assert.equal(f.hooks.size, 0);
});
