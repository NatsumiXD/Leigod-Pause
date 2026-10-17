'use strict';
// Pinned to GateProxyEx SHA256 aca4e862...d750153 (ia32). No target calls,
// argument edits, return-value edits, packet reads, or start/stop invocation.
const connections = new Map();
const udpFrames = new Map();
const listeners = [];
let gate = null;
let busy = 0;
let complete = false;
let moduleError = null;
let attachedAt = Date.now();
let dropped = 0;

function endpoint(args) { return args[1].toUInt32() + ':' + args[0].toUInt32(); }
function safe(fn, fallback = null) { try { return fn(); } catch (_) { return fallback; } }
function loopback(p) {
  const family = p.readU16();
  if (family === 2) return p.add(4).readU8() === 127;
  if (family === 23) {
    const bytes = new Uint8Array(p.add(8).readByteArray(16));
    return (bytes.slice(0, 15).every(x => x === 0) && bytes[15] === 1) ||
      (bytes.slice(0, 10).every(x => x === 0) && bytes[10] === 255 && bytes[11] === 255 && bytes[12] === 127);
  }
  return null;
}
function redirected(before, after) {
  return before === false && loopback(after);
}
function addConnection(key, pid, protocol, evidence) {
  if (!Number.isInteger(pid) || pid <= 0) return;
  if (connections.size >= 65536 && !connections.has(key)) { dropped++; complete = false; return; }
  connections.set(key, { pid, protocol, evidence, last_seen_ms: Date.now() });
}
function actualExport(module, name) {
  let p = module.getExportByName(name);
  // The exported connection query is a JMP thunk; internal callers use its target.
  for (let i = 0; i < 4 && p.readU8() === 0xe9; i++) p = p.add(5 + p.add(1).readS32());
  return p;
}
function hook(p, callbacks) { listeners.push(Interceptor.attach(p, callbacks)); }
function install(module) {
  if (gate || module.name.toLowerCase() !== 'gateproxyex.dll') return;
  if (Process.arch !== 'ia32') { moduleError = 'Unsupported GateProxy architecture'; return; }
  if (module.path.replace(/\\/g, '/').toLowerCase() !== MONITOR_CONFIG.modulePath.replace(/\\/g, '/').toLowerCase()) {
    moduleError = 'Loaded GateProxy path differs from the verified DLL'; return;
  }
  gate = module;
  moduleError = null;
  try {
    hook(module.getExportByName('start_proxy'), {
      onEnter() { busy++; },
      onLeave(retval) {
        busy--;
        // State comes from the runtime ID below; success alone is insufficient.
        if ((retval.toInt32() & 0xff) !== 0) send({event: 'start_returned'});
      }
    });
    hook(module.getExportByName('stop_proxy'), {
      onEnter() { busy++; complete = false; },
      onLeave() { busy--; connections.clear(); dropped = 0; complete = true; send({event: 'stop_returned'}); }
    });
    hook(module.getExportByName('tcpConnectRequest'), {
      onEnter(args) {
        this.key = 'tcp:' + endpoint(args); this.info = args[2];
        this.pid = safe(() => this.info.add(4).readU32());
        this.before = safe(() => loopback(this.info.add(39)));
      },
      onLeave() {
        if (safe(() => redirected(this.before, this.info.add(39)), false))
          addConnection(this.key, this.pid, 'tcp', 'remote_rewritten_to_loopback');
      }
    });
    hook(module.getExportByName('tcpClosed'), { onEnter(args) { connections.delete('tcp:' + endpoint(args)); } });
    hook(module.getExportByName('udpSend'), {
      onEnter(args) {
        this.frame = { key: 'udp:' + endpoint(args), pid: null };
        const stack = udpFrames.get(this.threadId) || [];
        stack.push(this.frame); udpFrames.set(this.threadId, stack);
      },
      onLeave(retval) {
        const stack = udpFrames.get(this.threadId);
        if (stack) { stack.pop(); if (!stack.length) udpFrames.delete(this.threadId); }
        if ((retval.toInt32() & 0xff) === 1)
          addConnection(this.frame.key, this.frame.pid, 'udp', 'udp_send_consumed_by_engine');
      }
    });
    hook(actualExport(module, 'getUdpConnInfo'), {
      onEnter(args) {
        this.output = args[2];
        const stack = udpFrames.get(this.threadId) || [];
        this.frame = stack[stack.length - 1];
      },
      onLeave(retval) {
        // NF_STATUS_SUCCESS == 0, packed NF_UDP_CONN_INFO starts with processId.
        if (this.frame && retval.toInt32() === 0) this.frame.pid = safe(() => this.output.readU32());
      }
    });
    hook(module.getExportByName('udpClosed'), { onEnter(args) { connections.delete('udp:' + endpoint(args)); } });
    send({event: 'hooks_ready', module: module.name});
  } catch (e) {
    for (const listener of listeners.splice(0)) listener.detach();
    moduleError = String(e); gate = null;
  }
}

function snapshot() {
  let state = 'unknown', reason = 'gate_module_not_loaded', runtimeId = null;
  if (moduleError) reason = moduleError;
  else if (gate && busy === 0) {
    const value = safe(() => {
      // IDA: start_proxy tests this runtime ID; stop path clears it. No locks
      // are acquired: read twice and reject a concurrent change.
      const slot = gate.base.add(0x9802a8);
      const p = slot.readPointer();
      if (p.isNull()) return null;
      const a = p.add(12).readU16(), b = p.add(12).readU16();
      return slot.readPointer().equals(p) && a === b ? a : null;
    });
    if (value !== null) {
      runtimeId = value; state = value === 0 ? 'inactive' : 'active'; reason = 'native_runtime_id';
      if (state === 'inactive') { connections.clear(); dropped = 0; complete = true; }
    } else reason = 'runtime_state_unreadable_or_changing';
  } else if (busy) reason = 'start_or_stop_in_progress';
  // Unknown state must not expose a previous active list as a current fact.
  return { state, reason, runtime_id: runtimeId, sdk_pid: Process.id,
    attached_at_ms: attachedAt, processes_complete: complete,
    coverage: 'GateProxy Netfilter TCP redirection and UDP consumption; observations since attach',
    dropped_connections: dropped,
    connections: state === 'active' ? Array.from(connections.values()) : [] };
}

const observer = Process.attachModuleObserver({
  onAdded: install,
  onRemoved(module) {
    if (gate && module.base.equals(gate.base)) {
      for (const listener of listeners.splice(0)) safe(() => listener.detach());
      gate = null; connections.clear(); complete = false; moduleError = 'GateProxy unloaded';
    }
  }
});
rpc.exports = { snapshot };
