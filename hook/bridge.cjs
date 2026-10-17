'use strict';

// The controller owns discovery and script source. This bridge only attaches to
// its explicit PID, transports events, and polls the script's read-only RPC.
const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
let startup = Promise.resolve();
let session;
let script;
let timer;
let configured = false;
let stopping = false;
let shutdown;
let exitCode = 0;

function emit(value) {
  if (!process.stdout.destroyed) process.stdout.write(JSON.stringify(value) + '\n');
}

function errorText(error) {
  return error instanceof Error ? error.message : String(error);
}

async function bounded(operation, milliseconds = 1500) {
  let deadline;
  try {
    await Promise.race([
      Promise.resolve().then(operation),
      new Promise((_, reject) => {
        deadline = setTimeout(() => reject(new Error('cleanup timed out')), milliseconds);
      }),
    ]);
  } catch (error) {
    process.stderr.write(errorText(error) + '\n');
  } finally {
    clearTimeout(deadline);
  }
}

function stop(code = 0) {
  exitCode = Math.max(exitCode, code);
  if (shutdown) return shutdown;
  stopping = true;
  clearTimeout(timer);
  // Closing the process connection also releases a session if attach itself
  // stalls. Try explicit script unload and detach first in every normal exit.
  const forceExit = setTimeout(() => process.exit(exitCode || 1), 5000);
  shutdown = (async () => {
    await startup.catch(() => {});
    if (script) await bounded(() => script.unload());
    if (session) await bounded(() => session.detach());
    input.close();
    clearTimeout(forceExit);
    process.exit(exitCode);
  })();
  return shutdown;
}

async function start(config) {
  if (typeof config.fridaModule !== 'string' || config.fridaModule.length === 0 ||
      !Number.isInteger(config.pid) || config.pid <= 0 ||
      typeof config.source !== 'string' || config.source.length === 0 ||
      !Number.isFinite(config.intervalMs) || config.intervalMs < 100) {
    throw new Error('Expected {fridaModule, pid, source, intervalMs >= 100}');
  }
  const fs = require('node:fs');
  const crypto = require('node:crypto');
  if (!config.verification || typeof config.verification.path !== 'string' ||
      config.verification.sha256 !== 'aca4e862d4f26073c99dfbe77bb750effbedfe805b6b5bac34b284220d750153') {
    throw new Error('A pinned GateProxy DLL verification is required');
  }
  const digest = crypto.createHash('sha256').update(fs.readFileSync(config.verification.path)).digest('hex');
  if (digest !== config.verification.sha256) throw new Error('Unsupported GateProxy DLL SHA256: ' + digest);
  const frida = require(config.fridaModule);
  const device = await frida.getLocalDevice();
  if (stopping) return;
  session = await device.attach(config.pid);
  session.detached.connect(reason => {
    emit({ kind: 'detached', reason });
    if (!stopping) void stop(1);
  });
  if (stopping) return;
  script = await session.createScript(config.source);
  script.logHandler = (level, message) => emit({ kind: 'script_log', level, message });
  script.message.connect(message => {
    if (message.type === 'send') {
      emit({ kind: 'hook', payload: message.payload });
    } else if (message.type === 'error') {
      emit({ kind: 'script_error', error: {
        description: message.description,
        stack: message.stack,
        fileName: message.fileName,
        lineNumber: message.lineNumber,
        columnNumber: message.columnNumber,
      } });
    }
    // Deliberately ignore the separate binary data argument.
  });
  if (stopping) return;
  await script.load();
  if (stopping) return;
  emit({ kind: 'attached', pid: config.pid });

  async function poll() {
    if (stopping) return;
    try {
      const snapshot = await script.exports.snapshot();
      if (!stopping) emit({ kind: 'snapshot', snapshot, pid: config.pid });
    } catch (error) {
      if (!stopping) {
        emit({ kind: 'script_error', error: errorText(error) });
        void stop(1);
      }
      return;
    }
    // Schedule only after RPC completion: snapshots never overlap.
    if (!stopping) timer = setTimeout(poll, config.intervalMs);
  }
  timer = setTimeout(poll, 0);
}

input.on('line', line => {
  if (stopping || line.trim().length === 0) return;
  try {
    const value = JSON.parse(line);
    if (!configured) {
      configured = true;
      startup = start(value);
      startup.catch(error => {
        emit({ kind: 'error', error: errorText(error) });
        void stop(1);
      });
    } else if (value.command === 'stop') {
      void stop();
    } else {
      emit({ kind: 'error', error: 'Only {command:"stop"} is accepted after configuration' });
    }
  } catch (error) {
    emit({ kind: 'error', error: errorText(error) });
    void stop(1);
  }
});
input.on('close', () => { void stop(); });
process.on('SIGINT', () => { void stop(); });
process.on('SIGTERM', () => { void stop(); });
process.stdout.on('error', error => {
  process.stderr.write(errorText(error) + '\n');
  void stop(1);
});
