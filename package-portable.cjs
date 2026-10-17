'use strict';
// Copies already installed, unmodified runtime files. No npm/install scripts run.
const fs=require('node:fs'),path=require('node:path'),{createRequire}=require('node:module');
const [exe,nodeExe,fridaDir,outputDirectory]=process.argv.slice(2);
if(!exe||!nodeExe||!fridaDir)throw Error('Usage: node package-portable.cjs EXE NODE_EXE FRIDA_DIRECTORY [OUTPUT_DIRECTORY]');
const out=path.resolve(outputDirectory||'dist/LeiGodLauncher'),runtime=path.join(out,'runtime');
fs.mkdirSync(path.join(runtime,'node_modules'),{recursive:true});
fs.copyFileSync(exe,path.join(out,'leigod-status.exe'));
fs.copyFileSync(nodeExe,path.join(runtime,'node.exe'));
for(const [source,destination]of [['Node-LICENSE','Node-LICENSE'],['Frida-COPYING','Frida-COPYING'],['Frida-COPYING.LIB','Frida-COPYING.LIB']])fs.copyFileSync(path.join('licenses',source),path.join(runtime,destination));
const packages=[],seen=new Set();
function copyPackage(directory){
 const pkg=JSON.parse(fs.readFileSync(path.join(directory,'package.json'),'utf8'));
 if(seen.has(pkg.name))return;seen.add(pkg.name);
 const destination=path.join(runtime,'node_modules',pkg.name);fs.mkdirSync(destination,{recursive:true});
 if(pkg.name==='frida'){
  for(const name of ['package.json','README.md','build','src','scripts','meson.build','Makefile','BSDmakefile','configure','configure.bat','make.bat','tsconfig.json','subprojects','releng','frida-bindgen']){
   const source=path.join(directory,name);if(fs.existsSync(source))fs.cpSync(source,path.join(destination,name),{recursive:true});
  }
 }else fs.cpSync(directory,destination,{recursive:true,filter:source=>!source.split(path.sep).slice(directory.split(path.sep).length).includes('node_modules')});
 packages.push({name:pkg.name,version:pkg.version,license:pkg.license,repository:pkg.repository});
 const resolve=createRequire(path.join(directory,'package.json'));
 for(const name of Object.keys(pkg.dependencies||{})){
  if(pkg.name==='frida'&&name==='prebuild-install')continue;
  let entry=resolve.resolve(name),folder=path.dirname(entry);
  while(folder!==path.dirname(folder)){
   const file=path.join(folder,'package.json');
   if(fs.existsSync(file)&&JSON.parse(fs.readFileSync(file,'utf8')).name===name)break;
   folder=path.dirname(folder);
  }
  copyPackage(folder);
 }
}
copyPackage(path.resolve(fridaDir));
fs.writeFileSync(path.join(out,'THIRD-PARTY.json'),JSON.stringify({node:{version:process.version,source:'https://github.com/nodejs/node',license:'MIT and bundled dependency licenses; see runtime/Node-LICENSE'},packages},null,2));
if(!fs.existsSync(path.join(out,'config.json')))fs.writeFileSync(path.join(out,'config.json'),JSON.stringify({exit_delay_minutes:10,auto_stop_enabled:true},null,2));
for(const file of ['README.md','live-validation.md'])fs.copyFileSync(file,path.join(out,file));
console.log(JSON.stringify({output:out,packages:packages.map(p=>`${p.name}@${p.version}`)}));
