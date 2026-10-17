'use strict';
/* Runtime attach, installation and Raft UI state capture are live-validated.
 * Initial callback availability still depends on normal application activity.
 * No fixed addresses, heap scans, thread migration, or return edits.
 * Prepend globalThis.LEIGOD_NAPI_CONFIG = {installSource, snapshotExpression,
 * intervalMs:1000, cleanupSource?}. installSource is a globally evaluated script;
 * false means retry later. snapshotExpression must synchronously return only
 * sanitized JSON data, never a Promise. Installers must be idempotent and supply
 * cleanupSource or a self-expiring lease if they install timers/subscriptions.
 *
 * napi_env is used only inside the original successful addon callback's thread.
 * Worker environments, finalizers and Frida timers are not JS entry points.
 * There is no guaranteed callback after attach/stop; report waiting/cleanup
 * failure rather than forcing a callback. Unloading Frida removes the native
 * interceptor; it cannot safely run main JS cleanup on an arbitrary thread.
 */
(() => {
  const config = globalThis.LEIGOD_NAPI_CONFIG;
  if (!config || typeof config.installSource !== 'string' ||
      typeof config.snapshotExpression !== 'string') {
    throw new Error('LEIGOD_NAPI_CONFIG requires installSource and snapshotExpression');
  }
  if (Process.platform !== 'windows' || !['ia32', 'x64'].includes(Process.arch)) {
    throw new Error('Only Windows ia32/x64 Node-API targets are supported');
  }
  const interval = Math.max(250, Number(config.intervalMs) || 1000);
  const maxBytes = 262144;
  const abi = Process.arch === 'ia32' ? 'mscdecl' : 'win64';
  const target = Process.getModuleByName('leigod.exe');
  const pointerSize = Process.pointerSize;
  const native = (name, args) => new NativeFunction(
    target.getExportByName(name), 'int', args, {abi, scheduling: 'cooperative'});
  const openScope = native('napi_open_handle_scope', ['pointer', 'pointer']);
  const closeScope = native('napi_close_handle_scope', ['pointer', 'pointer']);
  const pendingException = native('napi_is_exception_pending', ['pointer', 'pointer']);
  const clearException = native('napi_get_and_clear_last_exception', ['pointer', 'pointer']);
  const createString = native('napi_create_string_utf8',
    ['pointer', 'pointer', 'size_t', 'pointer']);
  const runScript = native('napi_run_script', ['pointer', 'pointer', 'pointer']);
  const readString = native('napi_get_value_string_utf8',
    ['pointer', 'pointer', 'pointer', 'size_t', 'pointer']);
  const busyThreads = new Set();
  let installedEnv = null;
  let lastAttempt = 0;
  let lastSnapshot = null;
  let lastSnapshotAt = null;
  let lastEntry = null;
  let stopRequested = false;
  let stopped = false;
  let cleanupWaiter = null;
  let cleanupResult = null;
  const listeners = [];
  const callbackCounts = {};
  const hookNames = [];

  const evalSource = source => `(0,eval)(${JSON.stringify(source)})`;
  function sourceFor(cleanup, installed) {
    const operation = cleanup
      ? (typeof config.cleanupSource === 'string'
          ? `${evalSource(config.cleanupSource)};return JSON.stringify({eligible:true,cleaned:true});`
          : 'return JSON.stringify({eligible:true,cleaned:false,error:"cleanupSource was not supplied"});')
      : `${installed ? '' : `if(${evalSource(config.installSource)}===false)return JSON.stringify({eligible:true,waiting:true});`}
         const value=${evalSource(config.snapshotExpression)};
         if(value&&typeof value.then==='function')throw new Error('Snapshot must be synchronous');
         return JSON.stringify({eligible:true,installed:true,snapshot:value});`;
    return `(()=>{try{
      const p=globalThis.process;
      if(!p||p.type!=='browser'||!p.versions?.electron||typeof p.mainModule?.require!=='function')
        return JSON.stringify({eligible:false});
      if(!p.mainModule.require('node:worker_threads').isMainThread)
        return JSON.stringify({eligible:false});
      ${config.transport ? 'globalThis.LEIGOD_MAIN_TRANSPORT='+JSON.stringify(config.transport)+';' : ''}
      ${operation}
    }catch(e){return JSON.stringify({eligible:true,error:String(e&&e.message||e)});}})()`;
  }
  const sourceBuffer = source => ({pointer:Memory.allocUtf8String(source),
    length:encodeURIComponent(source).replace(/%[a-f0-9]{2}/gi, 'x').length});
  const sourceBuffers = {
    install: sourceBuffer(sourceFor(false, false)),
    snapshot: sourceBuffer(sourceFor(false, true)),
    cleanup: sourceBuffer(sourceFor(true, true)),
  };

  function hasPending(env) {
    const out = Memory.alloc(1);
    if (pendingException(env, out) !== 0) throw new Error('Cannot check pending exception');
    return out.readU8() !== 0;
  }
  function checked(status, name) {
    if (status !== 0) throw new Error(`${name} returned napi_status ${status}`);
  }
  function evaluate(env, source) {
    const scopeOut = Memory.alloc(pointerSize);
    let scope = null;
    // Caller already checked there is no preexisting exception. Any exception
    // created below belongs to this probe and must not leak into the addon.
    try {
      checked(openScope(env, scopeOut), 'napi_open_handle_scope');
      scope = scopeOut.readPointer();
      const scriptOut = Memory.alloc(pointerSize);
      checked(createString(env, source.pointer, source.length, scriptOut), 'napi_create_string_utf8');
      const resultOut = Memory.alloc(pointerSize);
      checked(runScript(env, scriptOut.readPointer(), resultOut), 'napi_run_script');
      const result = resultOut.readPointer();
      const lengthOut = Memory.alloc(pointerSize);
      checked(readString(env, result, NULL, 0, lengthOut), 'napi_get_value_string_utf8(length)');
      const length = pointerSize === 4 ? lengthOut.readU32() : Number(lengthOut.readU64());
      if (!Number.isSafeInteger(length) || length > maxBytes) throw new Error('Snapshot exceeded size limit');
      const text = Memory.alloc(length + 1);
      checked(readString(env, result, text, length + 1, lengthOut), 'napi_get_value_string_utf8');
      return JSON.parse(text.readUtf8String(length));
    } finally {
      try {
        if (hasPending(env)) {
          const discarded = Memory.alloc(pointerSize);
          checked(clearException(env, discarded), 'napi_get_and_clear_last_exception');
        }
      } finally {
        if (scope !== null) checked(closeScope(env, scope), 'napi_close_handle_scope');
      }
    }
  }

  function attachEntry(name, requireAddonCaller) {
    let address;
    try { address = target.getExportByName(name); } catch (_) { return; }
    hookNames.push(name);
    callbackCounts[name] = 0;
    listeners.push(Interceptor.attach(address, {
    onEnter(args) {
      callbackCounts[name]++;
      // get_cb_info originates in addon callbacks. call_function/make_callback
      // are themselves documented JS-thread entry APIs; Node may call them
      // internally (e.g. TSFN delivery), so their caller need not be a .node.
      const caller = requireAddonCaller ? Process.findModuleByAddress(this.returnAddress) : null;
      const validCaller = !requireAddonCaller || (caller && /\.node$/i.test(caller.name));
      this.probe = !stopped && validCaller &&
        !busyThreads.has(this.threadId) && Date.now() - lastAttempt >= interval;
      if (this.probe) this.env = args[0];
    },
    onLeave(retval) {
      if (!this.probe || retval.toInt32() !== 0 || this.env.isNull() ||
          busyThreads.has(this.threadId)) return;
      busyThreads.add(this.threadId);
      lastAttempt = Date.now();
      try {
        if (hasPending(this.env)) return;
        const envKey = this.env.toString(); // Identity comparison, never reused as a pointer.
        const source = stopRequested ? sourceBuffers.cleanup :
          (installedEnv === envKey ? sourceBuffers.snapshot : sourceBuffers.install);
        const result = evaluate(this.env, source);
        if (!result.eligible) return;
        lastEntry = name;
        if (result.error) {
          send({event: 'napi_error', error: result.error});
          if (!stopRequested) return;
        }
        if (stopRequested) {
          stopped = true;
          const outcome = {cleaned: result.cleaned === true, error: result.error || null};
          cleanupResult = outcome;
          send({event: 'stopped', ...outcome});
          if (cleanupWaiter) { cleanupWaiter(outcome); cleanupWaiter = null; }
          for (const listener of listeners) listener.detach();
          return;
        }
        if (result.installed && installedEnv !== envKey) {
          installedEnv = envKey;
          send({event: 'ready', transport: 'napi', pid: Process.id, entry:name});
        }
        if (result.installed) {
          lastSnapshot = result.snapshot === undefined ? null : result.snapshot;
          lastSnapshotAt = Date.now();
          send({event: 'snapshot', snapshot: lastSnapshot, pid: Process.id});
        }
      } catch (error) {
        send({event: 'napi_error', error: String(error.message || error)});
      } finally {
        busyThreads.delete(this.threadId);
        this.env = null;
      }
    },
    }));
  }
  attachEntry('napi_get_cb_info', true);
  attachEntry('napi_call_function', false);
  attachEntry('napi_make_callback', false);
  if (listeners.length === 0) throw new Error('No supported exported Node-API callback entry');
  send({event: 'waiting', transport: 'napi',
    hooks:hookNames, reason: 'Waiting for a successful Node-API JS callback'});
  rpc.exports = {
    snapshot() { return {transport:'napi', installed: installedEnv !== null,
      stopped, observed_at_ms:lastSnapshotAt, snapshot:lastSnapshot,
      last_entry:lastEntry, hooks:hookNames.slice(), callback_counts:{...callbackCounts}}; },
    stop() {
      if (stopped) return Promise.resolve(cleanupResult);
      stopRequested = true;
      return new Promise(resolve => {
        cleanupWaiter = resolve;
        setTimeout(() => {
          if (cleanupWaiter === resolve) {
            cleanupWaiter = null;
            resolve({cleaned:false, error:'No eligible JS callback for cleanup within 3 seconds'});
          }
        }, 3000);
      });
    },
  };
})();
