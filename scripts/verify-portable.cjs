'use strict';
// Self-contained CI check: no LeiGod installation, login or elevation required.
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),{execFileSync}=require('node:child_process');
const root=path.resolve(process.argv[2]||'dist/LeiGodLauncher');
function checkX64(file){
 const bytes=fs.readFileSync(file);assert.equal(bytes.toString('ascii',0,2),'MZ',file);
 const offset=bytes.readUInt32LE(0x3c);assert.equal(bytes.toString('ascii',offset,offset+4),'PE\0\0',file);
 assert.equal(bytes.readUInt16LE(offset+4),0x8664,'Expected Windows x64: '+file);
}
for(const relative of ['leigod-status.exe','runtime/node.exe','runtime/node_modules/frida/build/frida_binding.node'])checkX64(path.join(root,relative));
for(const relative of ['README.md','THIRD-PARTY.json','runtime/Node-LICENSE','runtime/Frida-COPYING','runtime/Frida-COPYING.LIB'])assert.ok(fs.statSync(path.join(root,relative)).size>0,relative);
assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root,'config.json'),'utf8')),{exit_delay_minutes:10,auto_stop_enabled:true});
assert.equal(fs.existsSync(path.join(root,'logs')),false,'Personal logs must not be packaged');
const env={...process.env,PATH:path.join(process.env.SystemRoot,'System32')};
for(const key of ['NODE_PATH','NODE_OPTIONS','LEIGOD_NODE','LEIGOD_FRIDA_MODULE'])delete env[key];
const output=execFileSync(path.join(root,'runtime/node.exe'),['-e',`
const frida=require(process.argv[1]);
frida.getLocalDevice().then(device=>device.enumerateProcesses()).then(processes=>{
 if(!processes.some(p=>p.pid===process.pid))throw Error('Frida could not enumerate its own process');
 console.log(JSON.stringify({node:process.version,fridaLoaded:true}));
}).catch(error=>{console.error(error.message);process.exitCode=1;});
`,path.join(root,'runtime/node_modules/frida')],{env,encoding:'utf8',windowsHide:true,timeout:30000});
const result=JSON.parse(output),metadata=JSON.parse(fs.readFileSync(path.join(root,'THIRD-PARTY.json'),'utf8'));
assert.equal(result.node,metadata.node.version);assert.equal(result.fridaLoaded,true);
console.log('Portable x64 files, fresh config, licenses and bundled Node/Frida verified');
