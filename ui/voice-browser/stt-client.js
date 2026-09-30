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
 if(!data||bits!==16)throw Error('La sala envió un WAV que este navegador no entiende.');
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
  this.worker=null;this.pending=new Map();this.nextId=0;this.runtime=null;this.socket=null;
  this.language='auto';this.enabled=false;this.generation=0;
 }
 _ensureWorker(){
  if(this.worker)return;
  this.worker=new Worker('/voice-browser/stt-worker.js?v='+encodeURIComponent(globalThis.sidevoiceBuildId||'dev'),{type:'module'});
  this.worker.onmessage=({data})=>{
   const request=this.pending.get(data.id);if(!request)return;
   if(data.type==='progress'){request.progress?.(data.progress);return}
   this.pending.delete(data.id);
   if(data.type==='error')request.reject(Error(data.error));
   else request.resolve(data.capabilities||data.runtime||data.result);
  };
  this.worker.onerror=error=>{
   for(const request of this.pending.values())request.reject(Error(error.message||'Fallo el worker de transcripcion'));
   this.pending.clear();
  };
 }
 _request(type,data={},progress,transfer=[]){
  this._ensureWorker();const id=++this.nextId;
  return new Promise((resolve,reject)=>{this.pending.set(id,{resolve,reject,progress});this.worker.postMessage({id,type,...data},transfer)});
 }
 async capabilities(){return this._request('capabilities')}
 _preparation(data){window.dispatchEvent(new CustomEvent('voice-preparation',{detail:{kind:'transcription',...data}}))}
 _progress(value){
  const numeric=Number(value?.progress),file=String(value?.file||'').split('/').pop();
  const downloading=value?.status==='progress'||value?.status==='download';
  this._preparation({phase:'loading',title:'Preparando transcripcion',text:(downloading?'Descargando':'Preparando')+(file?' - '+file:''),progress:Number.isFinite(numeric)?numeric:null});
 }
 async prepare({model,device}){
  this._preparation({phase:'loading',title:'Preparando transcripcion',text:'Comprobando el motor local...',progress:null});
  try{this.runtime=await this._request('load',{model,device},value=>this._progress(value));this._preparation({phase:'ready'});return this.runtime}
  catch(error){this._preparation({phase:'error',title:'No se pudo preparar la transcripcion',text:error.message,progress:null});throw error}
 }
 start({socket,language='auto'}){
  this.socket=socket;this.language=language;this.enabled=true;this.generation++;
 }
 stop(){
  this.enabled=false;this.socket=null;this.generation++;
  const error=new DOMException('Transcripción cancelada','AbortError');
  for(const request of this.pending.values())request.reject(error);this.pending.clear();
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
   this._preparation({phase:'inline',text:'Transcribiendo en este navegador...'});
   const language=request.language||(this.language==='auto'?null:this.language);
   const result=await this._request('transcribe',{audio:audio.buffer,model:this.runtime.model,device:this.runtime.device,language:language||'auto'},null,[audio.buffer]);
   if(generation!==this.generation)return;
   if(!usefulTranscript(result.text))throw Error('El modelo produjo una transcripción degenerada. Inténtalo de nuevo.');
   this._send('voice-transcript',{session_id,request_id,text:result.text,metrics:{audio_ms,recognition_ms:Math.round(result.elapsed_ms),request_to_transcript_ms:Math.round(performance.now()-started),device:result.device,model:result.model}});
   this._preparation({phase:'hidden'});
  }catch(error){
   if(generation!==this.generation)return;
   this._send('voice-transcript-error',{session_id,request_id,error:error.message});
   this._preparation({phase:'error',title:'No se pudo transcribir',text:error.message,progress:null});
  }
 }
}
window.roomTranscription=new BrowserTranscription();
