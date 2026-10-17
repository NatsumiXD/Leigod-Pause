'use strict';
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),{execFileSync}=require('node:child_process');
const root=path.resolve('dist/LeiGodLauncher'),node=path.join(root,'runtime/node.exe'),frida=path.join(root,'runtime/node_modules/frida');
const env={...process.env,PATH:path.join(process.env.SystemRoot,'System32')};delete env.LEIGOD_NODE;delete env.LEIGOD_FRIDA_MODULE;delete env.NODE_PATH;delete env.NODE_OPTIONS;
const debug=path.join(root,'diagnostic-fixture.exe');
try{
 fs.copyFileSync(path.resolve('target/launcher/debug/leigod-status.exe'),debug);
 const diagnostic=JSON.parse(execFileSync(debug,['--diagnose'],{env,encoding:'utf8',windowsHide:true}));
 assert.equal(path.resolve(diagnostic.node).toLowerCase(),node.toLowerCase());assert.equal(path.resolve(diagnostic.frida).toLowerCase(),frida.toLowerCase());
 console.log('PATH excludes installed Node; launcher selected both bundled dependencies');
 const text=execFileSync(node,[path.resolve('tests/bridge-smoke.cjs'),frida,
  path.join(path.dirname(diagnostic.installation),'resources/leishenSdk/GateProxyEx.dll'),path.resolve('target/refresh-fixture/leigod.exe')],{env,encoding:'utf8',windowsHide:true,timeout:20000});
 process.stdout.write(text);
}finally{if(fs.existsSync(debug))fs.unlinkSync(debug);}
