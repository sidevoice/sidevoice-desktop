function usefulTranscript(value){
 const text=String(value||'').trim(),compact=text.replace(/\s/g,'');
 if(!text||(!/[\p{L}\p{N}]/u.test(text)&&compact.length>=8))return false;
 if(compact.length>=24&&new Set(compact).size<=3)return false;
 const tokens=text.split(/\s+/);
 if(tokens.length>=10&&new Set(tokens).size/tokens.length<.15)return false;
 return true;
}
/* The room sends a finished turn as a 16-bit PCM WAV; Whisper wants float samples at 16 kHz. */
function decodeWav(base64){
 const bytes=Uint8Array.from(atob(base64),c=>c.charCodeAt(0)),view=new DataView(bytes.buffer);
 if(bytes.length<12||String.fromCharCode(...bytes.subarray(0,4))!=='RIFF'||String.fromCharCode(...bytes.subarray(8,12))!=='WAVE')throw Error('La sala envió un audio que no es WAV.');
 let offset=12,channels=1,rate=16000,bits=16,data=null;
 while(offset+8<=bytes.length){
  const id=String.fromCharCode(...bytes.subarray(offset,offset+4)),size=view.getUint32(offset+4,true),body=offset+8;
  if(id==='fmt '){channels=view.getUint16(body+2,true);rate=view.getUint32(body+4,true);bits=view.getUint16(body+14,true)}
  else if(id==='data'){data=bytes.subarray(body,Math.min(bytes.length,body+size));break}
  offset=body+size+(size%2);
 }
 if(!data||bits!==16)throw Error('La sala envió un WAV que este dispositivo no entiende.');
 const frames=Math.floor(data.length/(2*channels)),mono=new Float32Array(frames),samples=new Int16Array(data.buffer,data.byteOffset,frames*channels);
 for(let i=0;i<frames;i++){let sum=0;for(let c=0;c<channels;c++)sum+=samples[i*channels+c];mono[i]=sum/channels/32768}
 if(rate===16000)return mono;
 const out=new Float32Array(Math.round(frames*16000/rate)),step=rate/16000;
 for(let i=0;i<out.length;i++){const p=i*step,j=Math.floor(p),a=mono[Math.min(j,frames-1)],b=mono[Math.min(j+1,frames-1)];out[i]=a+(b-a)*(p-j)}
 return out;
}
/* Whisper in this browser, as a transcription provider the room calls: it never decides where a turn ends. */
class BrowserTranscription{
 constructor(){
  this.worker=null;this.held=null;this.retired=new Set();this.pending=new Map();this.nextId=0;this.runtime=null;this.socket=null;
  this.language='auto';this.enabled=false;this.generation=0;
 }
 /* A native build: the desktop app's own engine (native-worker.js), same protocol; a page one: Whisper in a Worker. */
 _ensureWorker(native=!!this.native){
  if(this.worker&&!!this.native===native)return;
  // Whatever was waiting on the worker being replaced will never be answered: say so rather than hang.
  for(const request of this.pending.values())request.reject(Error('Se cambió el motor de transcripción.'));this.pending.clear();
  this.worker?.terminate();this.worker=null;
  this.worker=this.candidate(native);this.native=native;this._listen(this.worker);
 }
 /* A worker of its own for a model check (sidevoice/sidevoice-core#21): the same protocol as this client's, loaded and checked apart
  * from the one a call is using, which keeps transcribing until the checked model takes its place (adopt). */
 candidate(native=false){
  if(!native)return new Worker('/voice-browser/stt-worker.js?v='+encodeURIComponent(globalThis.sidevoiceBuildId||'dev'),{type:'module'});
  const worker=globalThis.sidevoiceNativeWorkers?.transcription?.()||null;
  if(!worker)throw Error('El motor nativo solo está en la app de escritorio de Sidevoice.');
  return worker;
 }
 /* A checked model takes the place of the one in use, already loaded: every request goes to its worker from now on.
  * Until the change is settled (a call may refuse it) the old worker is held: `commit()` lets it go — once what it was
  * still doing is over, and in a page with the memory it held — and `restore()` puts it back and lets the new one go. */
 adopt(worker,runtime,native=false){
  const held={worker:this.worker,runtime:this.runtime,native:this.native};
  this.held=held;this.worker=worker;this.native=!!native;this.runtime=runtime;this._listen(worker);
  return {
   commit:()=>{if(this.held===held)this.held=null;this._retire(held.worker)},
   restore:()=>{
    if(this.held!==held||this.worker!==worker)return;
    this.held=null;this.worker=held.worker;this.runtime=held.runtime;this.native=held.native;
    if(held.worker)this._listen(held.worker);
    for(const [id,request] of this.pending)if(request.worker===worker){this.pending.delete(id);request.reject(new DOMException('Se volvió al modelo anterior.','AbortError'))}
    this._retire(worker);
   },
  };
 }
 _listen(worker){
  worker.onmessage=({data})=>{
   const request=this.pending.get(data.id);
   // A late answer nobody waits for any more (a stop cleared it) is still the moment to let an old worker go.
   if(!request||request.worker!==worker){if(this.retired.has(worker))this._retire(worker);return}
   if(data.type==='progress'){request.progress?.(data.progress);return}
   this.pending.delete(data.id);
   if(data.type==='error')request.reject(Object.assign(Error(data.error),{step:data.step,reason:data.reason}));
   else request.resolve(data.capabilities||data.runtime||data.result);
   this._retire(worker);
  };
  worker.onerror=error=>{
   for(const [id,request] of this.pending)if(request.worker===worker){this.pending.delete(id);request.reject(Error(error.message||'Fallo el worker de transcripcion'))}
   this._retire(worker);
  };
 }
 /* A worker that is no longer the one in use (nor held) goes once nothing waits on it; until then it is kept here, so
  * clearing what it was doing (stop) or its late answer lets it go too. */
 _retire(worker){
  if(!worker||worker===this.worker||worker===this.held?.worker)return;
  if([...this.pending.values()].some(request=>request.worker===worker)){this.retired.add(worker);return}
  this.retired.delete(worker);worker.terminate();
 }
 _request(type,data={},progress,transfer=[]){
  this._ensureWorker(data.native);const id=++this.nextId,worker=this.worker;
  return new Promise((resolve,reject)=>{this.pending.set(id,{resolve,reject,progress,worker});worker.postMessage({id,type,...data},transfer)});
 }
 /* What this browser can run in the page (never the native engine's list: that one is the app's). */
 async capabilities(){
  if(!this.native)return this._request('capabilities',{native:false});
  // The native engine is in use: ask the page's own engine in a Worker of its own, so a transcription in flight
  // on the native one is never cut short by someone opening the settings.
  const probe=new Worker('/voice-browser/stt-worker.js?v='+encodeURIComponent(globalThis.sidevoiceBuildId||'dev'),{type:'module'});
  try{return await new Promise((resolve,reject)=>{probe.onmessage=({data})=>data.type==='error'?reject(Error(data.error)):data.type==='capabilities'&&resolve(data.capabilities);probe.onerror=e=>reject(Error(e.message||'capabilities'));probe.postMessage({id:1,type:'capabilities'})})}
  finally{probe.terminate()}
 }
 _preparation(data){window.dispatchEvent(new CustomEvent('voice-preparation',{detail:{kind:'transcription',...data}}))}
 _progress(value){
  const numeric=Number(value?.progress),file=String(value?.file||'').split('/').pop();
  const downloading=value?.status==='progress'||value?.status==='download';
  this._preparation({phase:'loading',title:'Preparando transcripcion',text:(downloading?'Descargando':'Preparando')+(file?' - '+file:''),progress:Number.isFinite(numeric)?numeric:null});
 }
 /* The build an offer chose: a catalogue model, its engine and accelerator, and whether that engine is native.
  * `progress` hears the engine's own progress events too (what the room's downloads count in bytes). */
 async prepare({model,engine,accelerator,native=false},progress){
  this._preparation({phase:'loading',title:'Preparando transcripcion',text:'Comprobando el motor local...',progress:null});
  try{this.runtime=await this._request('load',{model,engine,accelerator,native},value=>{this._progress(value);progress?.(value)});this._preparation({phase:'ready'});return this.runtime}
  catch(error){
   // A cancel — this page's, or the desktop app's (install_cancelled, load_cancelled) — ends the preparation; it is
   // not a failure to show (N03).
   const cancelled=error?.name==='AbortError'||['install_cancelled','load_cancelled'].includes(error?.reason?.key);
   this._preparation(cancelled?{phase:'hidden'}:{phase:'error',title:'No se pudo preparar la transcripcion',text:error.message,progress:null});throw error;
  }
 }
 /* What the worker was loading is dropped, download included: the worker goes, and the next request makes another. */
 abandon(){
  const worker=this.worker;this.worker=null;this.runtime=null;
  for(const request of this.pending.values())request.reject(new DOMException('Transcripción cancelada','AbortError'));this.pending.clear();
  worker?.terminate();
 }
 start({socket,language='auto'}){
  this.socket=socket;this.language=language;this.enabled=true;this.generation++;
 }
 stop(){
  this.enabled=false;this.socket=null;this.generation++;
  const error=new DOMException('Transcripción cancelada','AbortError');
  for(const request of this.pending.values())request.reject(error);this.pending.clear();
  for(const worker of [...this.retired])this._retire(worker);
  this.worker?.postMessage({type:'cancel'});
 }
 _send(type,data){if(this.socket?.readyState===WebSocket.OPEN)this.socket.send(JSON.stringify({type,data}))}
 /* One finished turn from the room: decode it, run Whisper, answer with the text or the reason. */
 async transcribe(request){
  const generation=this.generation,session_id=window.sidevoiceSessionId?.(),request_id=request?.request_id;
  if(!this.enabled||!this.socket||!request_id)return;
  if(!this.runtime){this._send('voice-transcript-error',{session_id,request_id,error:'El modelo de transcripción no está preparado.'});return}
  const started=performance.now();
  try{
   const audio=decodeWav(request.audio_base64),audio_ms=Math.round(audio.length/16);
   this._preparation({phase:'inline',text:'Transcribiendo en este dispositivo...'});
   const language=request.language||(this.language==='auto'?null:this.language);
   const result=await this._request('transcribe',{audio:audio.buffer,model:this.runtime.model,engine:this.runtime.engine,accelerator:this.runtime.accelerator,native:!!this.native,language:language||'auto'},null,[audio.buffer]);
   if(generation!==this.generation)return;
   if(!usefulTranscript(result.text))throw Error('El modelo produjo una transcripción degenerada. Inténtalo de nuevo.');
   this._send('voice-transcript',{session_id,request_id,text:result.text,metrics:{audio_ms,recognition_ms:Math.round(result.elapsed_ms),request_to_transcript_ms:Math.round(performance.now()-started),accelerator:result.accelerator,model:result.model}});
   this._preparation({phase:'hidden'});
  }catch(error){
   if(generation!==this.generation)return;
   this._send('voice-transcript-error',{session_id,request_id,error:error.message});
   this._preparation({phase:'error',title:'No se pudo transcribir',text:error.message,progress:null});
  }
 }
}
window.roomTranscription=new BrowserTranscription();
