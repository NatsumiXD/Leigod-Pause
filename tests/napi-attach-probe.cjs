'use strict';
// Capability probe only: no NAPI functions or app APIs are invoked.
const frida=require(require('node:path').resolve(process.argv[2]));
function report(value){console.log(JSON.stringify(value));if(process.argv[4])require('node:fs').writeFileSync(process.argv[4],JSON.stringify(value));}
const timer=setTimeout(()=>{report({error:'attach timeout'});process.exit(2);},12000);
(async()=>{
  let session;
  try {
    session=await (await frida.getLocalDevice()).attach(Number(process.argv[3]));
    const script=await session.createScript(`send({pid:Process.id,arch:Process.arch,napi:Process.mainModule.enumerateExports().filter(e=>['napi_get_cb_info','napi_run_script'].includes(e.name)).map(e=>e.name)});`);
    script.message.connect(m=>report(m.type==='send'?m.payload:m));
    await script.load();await script.unload();await session.detach();clearTimeout(timer);process.exit(0);
  }catch(error){report({error:error.message});if(session)await session.detach();clearTimeout(timer);process.exit(1);}
})();
