/* The desktop app's native engine behind the same message protocol as this package's workers, so the
 * transcription and voice clients do not change: they ask for the device 'native' and get one of these
 * instead of a Web Worker. The engine is the app's (`window.__sidevoiceDesktop.host.nativeEngine`,
 * sidevoice/sidevoice-desktop docs/ENGINES.md): the same models the page runs, downloaded once and run on the
 * machine's own hardware. Model ids here are the page's (a Hugging Face repository); the app maps them to its
 * catalog's (`available().pageIds`). */
(function(){
 function host(){return globalThis.__sidevoiceDesktop?.host?.nativeEngine||null}
 /* The same chunks the in-page voice cuts a text into (engine.js splitText): a turn is spoken as it is synthesized. */
 function splitText(text,limit=160){const chunks=[];let current='';for(const word of String(text||'').trim().split(/\s+/).filter(Boolean)){if(current&&(current.length+word.length+1)>limit){chunks.push(current);current=''}current+=(current?' ':'')+word;if(/[.!?;:]$/.test(word)){chunks.push(current);current=''}}if(current)chunks.push(current);return chunks}
 async function catalogId(engine,pageId,task){
  const info=await engine.available();
  const id=info.pageIds?.[pageId]||pageId;
  const offer=(info.offers||[]).find(item=>item.model===id&&item.task===task);
  if(!offer)throw Error('Este modelo no se puede ejecutar de forma nativa en este equipo.');
  return {id,offer};
 }
 async function ready(engine,offer,progress){
  if(offer.installed)return;
  await engine.install(offer.model,offer.engine,(done,total)=>progress({status:'progress',progress:total?done*100/total:0,file:offer.label}));
 }
 /* Posts to whoever set onmessage, asynchronously, like a Worker. */
 class Stand{
  constructor(engine){this.engine=engine;this.onmessage=null;this.onerror=null;this.epoch=0;this.chain=Promise.resolve()}
  emit(data){setTimeout(()=>this.onmessage?.({data}),0)}
  terminate(){this.epoch++}
  postMessage(data){
   if(data?.type==='cancel'){this.epoch++;return}
   const epoch=this.epoch;
   this.chain=this.chain.then(()=>this.handle(data,epoch)).catch(error=>{if(epoch===this.epoch)this.emit({id:data.id,type:'error',error:String(error?.message||error)})});
  }
 }
 /* stt-worker.js's protocol: capabilities, load {model}, transcribe {audio, model, language}. */
 class NativeTranscription extends Stand{
  async handle(data,epoch){
   const send=(type,extra={})=>{if(epoch===this.epoch)this.emit({id:data.id,type,...extra})};
   if(data.type==='capabilities'){const info=await this.engine.available();send('capabilities',{capabilities:{native:true,webgpu:false,webgpuFp16:false,wasm:false,
    models:Object.entries(info.pageIds||{}).filter(([,id])=>(info.offers||[]).some(o=>o.model===id&&o.task==='stt')).map(([page])=>page)}});return}
   if(data.type==='load'){const {id,offer}=await catalogId(this.engine,data.model,'stt');await ready(this.engine,offer,progress=>send('progress',{progress}));this.model=id;send('ready',{runtime:{device:'native',model:data.model,cached:offer.installed}});return}
   if(data.type==='transcribe'){
    const started=performance.now(),model=this.model||(await catalogId(this.engine,data.model,'stt')).id;
    const text=await this.engine.transcribe(model,new Float32Array(data.audio),16000,data.language&&data.language!=='auto'?data.language:'');
    send('result',{result:{text:String(text||'').trim(),elapsed_ms:performance.now()-started,device:'native',model:data.model}});return;
   }
   throw Error('Mensaje de transcripcion desconocido');
  }
 }
 /* worker.js's protocol: load, speak {text, voice, speed} → audio chunks, done. */
 class NativeVoice extends Stand{
  async handle(data,epoch){
   const send=(type,extra={})=>{if(epoch===this.epoch)this.emit({id:data.id,type,...extra})};
   if(!this.model){const {id,offer}=await catalogId(this.engine,'onnx-community/Kokoro-82M-v1.0-ONNX','tts');await ready(this.engine,offer,progress=>send('progress',{progress}));this.model=id;send('ready',{device:'native'})}
   else if(data.type==='load')send('ready',{device:'native'});
   if(data.type==='load')return;
   for(const text of splitText(data.text)){
    if(epoch!==this.epoch)return;
    const started=performance.now(),audio=await this.engine.synthesize(this.model,data.voice,data.speed||1,text);
    if(epoch!==this.epoch)return;
    send('audio',{text,samples:audio.samples,sampleRate:audio.sampleRate,phonemes:'',elapsedMs:performance.now()-started,device:'native'});
   }
   send('done');
  }
 }
 globalThis.sidevoiceNativeWorkers={
  available:()=>!!host(),
  transcription:()=>host()?new NativeTranscription(host()):null,
  voice:()=>host()?new NativeVoice(host()):null,
  splitText,
 };
})();
