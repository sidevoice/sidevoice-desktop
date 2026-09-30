// Provider alignment is mapped to original UTF-16 text offsets. Never guess word timing.
function alignedWordCues(text,alignment,duration){
 if(typeof text!=='string'||!alignment)return [];
 const chars=alignment.characters,starts=alignment.character_start_times_seconds,ends=alignment.character_end_times_seconds;
 if(!Array.isArray(chars)||!Array.isArray(starts)||!Array.isArray(ends)||chars.length!==starts.length||chars.length!==ends.length)return [];
 if(!chars.every(c=>typeof c==='string'&&c.length>0)||chars.join('')!==text)return [];
 const startAt=[],endAt=[];let previous=0;
 for(let i=0;i<chars.length;i++){
  const start=starts[i],end=ends[i];
  if(!Number.isFinite(start)||!Number.isFinite(end)||start<previous||end<start||end>duration+.25)return [];
  for(let j=0;j<chars[i].length;j++){startAt.push(start);endAt.push(end)}
  previous=start;
 }
 const cues=[];
 for(const word of text.matchAll(/\S+/gu)){
  const first=word.index,last=first+word[0].length;
  if(endAt[last-1]>startAt[first])cues.push({from:first,to:last,start:startAt[first],end:endAt[last-1],mode:'word'});
 }
 return cues;
}
function chunkTextRange(text,chunk,from=0){
 if(typeof text!=='string'||typeof chunk!=='string'||!chunk.trim())return null;
 const words=chunk.trim().split(/\s+/u).map(word=>word.replace(/[.*+?^${}()|[\]\\]/g,'\\$&'));
 const match=new RegExp(words.join('\\s+'),'u').exec(text.slice(from));
 return match?{from:from+match.index,to:from+match.index+match[0].length}:null;
}
/* Local synthesis and playout. A canceled job can never emit late audio. */
/* iPhone and iPad (which reports itself as a Mac with a touch screen). Only there does a page that goes away
 * have to let go of its output: elsewhere a background tab keeps sounding, like any call or music tab (#96). */
function pausesWhileHidden(){
 const nav=typeof navigator!=='undefined'?navigator:null;if(!nav)return true;
 return /iPad|iPhone|iPod/.test(nav.userAgent||'')||(nav.platform==='MacIntel'&&nav.maxTouchPoints>1);
}
/* How the room's voice reaches the speaker, per platform (#80). iPhone and iPad need it to leave through a
 * media element: only that is part of their echo-cancellation reference. Everywhere else the element is a
 * liability — Chrome on the Mac keeps the context's clock and the element's device clock in step by
 * resampling slightly faster or slower, which is heard as the voice going deep, normal, then high — so the
 * context plays straight to its own destination, through one gain this engine owns. A device can force
 * either with localStorage "sidevoice.output" = "element" | "context", to compare the two by ear. */
function outputStrategy(){
 try{const forced=globalThis.localStorage?.getItem?.('sidevoice.output');if(forced==='element'||forced==='context')return forced}catch{}
 return pausesWhileHidden()?'element':'context';
}
class RoomVoice {
 constructor(){this.worker=null;this.context=null;this.output=null;this.job=null;this.serial=0;this.device=null;this.ready=false;this.outputDeviceId='default';this.resuming=null;
  // What the output did lately, for the stats dialog: a stuck buzz on a phone is otherwise invisible from here.
  this.events=[];this.stalls=0;this.stallCheckMs=500;this.stallAfterMs=700;this.stallLimit=3;this.awayLimitMs=30000;this.pausesByDefault=pausesWhileHidden();this.outputStrategy=outputStrategy();this.master=null;this.pauseWhileHidden=this.pausesByDefault;this.audible=true;this.keepAlive=null;this.keepAliveWanted=false;this.keepAliveLevel=1e-4;this.tailSeconds=1.5;
  // The ambient bed (#42) is a loop of its own, and never the first thing a fresh output renders.
  this.greetSeconds=1.8;this.greetedAt=0;this.rendered=false;
  this.presence=null;this.presencePulseSeconds=3.6;
  /* Phrases of breaths with silence between them, as [breaths, seconds of silence after]: a bed that never
   * stops becomes noise over a long turn (#91). Uneven on purpose, so the loop does not read as a metronome. */
  this.presencePhrases=[[2,3],[1,4],[2,2.5]];
  // Many silences are a second or two long: a breath that starts from nothing is gone before it is heard,
  // so the bed begins a third of the way into its first swell (2026-09-26, in the car).
  this.presenceStartOffset=.35;this.presenceFadeSeconds=.6;this.presenceMaxVolume=.2}
 note(kind,detail){const event={at:Date.now(),kind,...(detail?{detail}:{})};this.events.push(event);if(this.events.length>24)this.events.shift();
  // The event list is bounded, so what it proves is kept apart from it: a voice has already left this output.
  if(kind==='play-encoded'||kind==='complete')this.rendered=true;
  if(typeof window!=='undefined'&&window.dispatchEvent&&typeof CustomEvent==='function'){try{window.dispatchEvent(new CustomEvent('voice-output',{detail:event}))}catch{}}}
 /* The output as it is right now, and what happened to it lately. */
 health(){
  const element=this.output?.element;
  return {context:this.context?.state||'none',clock:this.context?Math.round(this.context.currentTime*1000)/1000:null,
   output:element?'element':(this.context?'context':'none'),strategy:this.outputStrategy,
   rate:this.context?.sampleRate||null,buffer_rate:this.lastBufferRate||null,
   element:element?{paused:!!element.paused,readyState:element.readyState??null}:null,
   playing:!!this.job?.playing,presence:this.presence?this.presence.volume:null,stalls:this.stalls,resuming:!!this.resuming,events:this.events.slice(-12)};
 }
 /* A voice that starts low and climbs is a rate that changed under it: the phone moved between its speaker
  * and a Bluetooth headset, or the buffer was made for a rate the output no longer has. The engine cannot
  * stop the route from moving, but it can say so instead of leaving someone guessing (2026-09-20). */
 watchRate(bufferRate){
  const rate=this.context?.sampleRate||null;
  if(bufferRate)this.lastBufferRate=bufferRate;
  if(rate&&this.knownRate&&rate!==this.knownRate)this.note('rate-changed',this.knownRate+' → '+rate);
  if(rate)this.knownRate=rate;
  if(rate&&bufferRate&&Math.abs(rate-bufferRate)>1&&!this.mismatchNoted){this.mismatchNoted=true;this.note('rate-mismatch',bufferRate+' en un contexto de '+rate)}
 }
 announce(text,phase='loading',progress=null){if(window.dispatchEvent)window.dispatchEvent(new CustomEvent('voice-preparation',{detail:{text,phase,progress}}))}
 async unlock(){this.context??=new AudioContext();await this.context.resume();if(this.context.state!=='running'){this.note('unlock-refused',this.context.state);throw Error('Permite reproducir audio en este navegador.')}await this.ensureOutput();this.greet()}
 /* A fresh output is greeted once: two soft notes, then a second and a half of silence, in one buffer.
  * The notes tell the person they are in and, being audible, keep the media element in the phone's echo
  * reference (half a second of silence alone took it out for the whole session, 2026-09-19). The silence
  * after them keeps the sink fed until the element has actually started: a bare quarter-second chime ended
  * before that and the element looped its last instant, the same failure as a cut without a tail. */
 greet(){
  if(!this.context||(!this.output&&!this.master)||this.greeted||typeof this.context.createBufferSource!=='function')return;
  this.greeted=true;this.greetedAt=Date.now();
  this.signal('connect');
  // Scheduling is not hearing. If the element is still not playing a moment later, the output never
  // reached the speaker — and an output the phone never heard is one its echo canceller ignores, which
  // is when the room starts interrupting itself. Hand the element its stream again and try once more.
  const check=setTimeout(()=>{
   const element=this.output?.element;
   if(!element||(this.context.state==='running'&&!element.paused)||this.greetRetried)return;
   this.greetRetried=true;   // one repair, not a loop: a second failure is the watchdog's business
   this.note('greet-unheard',this.context.state+' · '+(element.paused?'paused':'playing'));
   this.greeted=false;
   Promise.resolve(this.resumeOutput()).then(()=>this.greet());
  },700);
  check?.unref?.();
 }
 /* The sound of a call opening and of one ending: two notes, rising to connect and falling to hang up, at a
  * level a phone speaker in a car actually carries, followed by silence in the same buffer so the element
  * is never left without a source the instant the notes end. Audible first, always: digital silence as the
  * first thing an element renders is what takes it out of the phone's echo reference (2026-09-19). */
 signal(kind='connect'){
  if(!this.context||typeof this.context.createBufferSource!=='function')return false;
  try{
   const rate=this.context.sampleRate||48000,seconds=kind==='connect'?(this.greetSeconds||1.8):1.2;
   // A retry is a reminder, not news: the same low note, once, and quieter.
   const level=kind==='retry'?.2:.42;
   const length=Math.round(rate*seconds),samples=new Float32Array(length);
   // Joining rises, hanging up falls; losing the room is two low notes that stay low, and getting it back
   // is the join's rise, quicker — each told apart by ear, with nobody looking at a screen.
   const notes={connect:[[523.25,0,.13],[783.99,.14,.22]],hangup:[[659.25,0,.13],[415.30,.14,.26]],
    lost:[[392,0,.12],[392,.2,.12]],retry:[[392,0,.08]],back:[[523.25,0,.09],[659.25,.1,.09],[783.99,.2,.16]]}[kind]||[];
   for(const [hz,at,dur] of notes){
    const from=Math.round(at*rate),n=Math.round(dur*rate);
    for(let i=0;i<n&&from+i<length;i++){
     const t=i/n,envelope=t<.12?(t/.12):Math.pow(Math.max(0,1-(t-.12)/.88),1.6);
     samples[from+i]+=envelope*(Math.sin(2*Math.PI*hz*i/rate)+.22*Math.sin(4*Math.PI*hz*i/rate));
    }
   }
   let peak=0;for(let i=0;i<length;i++)peak=Math.max(peak,Math.abs(samples[i]));
   if(peak>0)for(let i=0;i<length;i++)samples[i]*=level/peak;
   const buffer=this.context.createBuffer(1,length,rate);buffer.copyToChannel(samples,0);
   const source=this.context.createBufferSource();source.buffer=buffer;source.connect(this.destination);
   source.start(this.context.currentTime+.2);this.note(kind==='connect'?'chime':kind==='hangup'?'hangup':'signal',kind==='connect'||kind==='hangup'?undefined:kind);
   return true;
  }catch(error){this.note('chime-failed',error?.message||'chime');return false}
 }
 /* The room's voice leaves through a media element, not the context's own output: on iOS Safari only
  * media-element playback is part of the echo-cancellation reference, so this is what lets the
  * microphone subtract our own voice instead of opening a turn with it. Falls back to the context. */
 async ensureOutput(){
  if(this.outputStrategy!=='element'){this.ensureMaster();return}
  if(this.output||typeof this.context.createMediaStreamDestination!=='function'||typeof Audio==='undefined')return;
  const sink=this.context.createMediaStreamDestination(),element=new Audio();
  element.srcObject=sink.stream;element.playsInline=true;element.autoplay=true;element.muted=!this.audible;
  try{await element.play()}catch(error){this.note('element-refused',error?.message||'play');return}
  this.output={sink,element};this.note('element-ready');
  // iOS interrupts the page's audio when the user pulls down notifications, switches apps, or the
  // microphone takes the audio route over — which is what an interruption while we speak looks
  // like in a car. An element left playing through that comes back as a stuck buzz, so it is
  // paused while the page is away, and everything is put back together when it returns. A desktop tab
  // in the background is not that: it keeps playing (#96).
  const settle=event=>{
   const hidden=typeof document!=='undefined'&&document.hidden;
   this.note(event?.type||'settle',(hidden?'hidden':'visible')+' · '+this.context.state);
   if((hidden&&this.pauseWhileHidden)||this.context.state!=='running')element.pause();
   this.ensureKeepAlive();
   // With the screen locked on purpose (#59), the end of an interruption — a phone call hung up — must put the
   // output back without waiting for the page to be shown: iOS already restarted the context, and a call
   // that stays paused until somebody unlocks the phone is a call nobody hears (2026-09-26).
   if(!hidden||(!this.pauseWhileHidden&&this.context.state==='running'))this.resumeOutput();
  };
  if(typeof document!=='undefined'&&document.addEventListener)document.addEventListener('visibilitychange',settle);
  if(typeof window!=='undefined'&&window.addEventListener){window.addEventListener('pagehide',settle);window.addEventListener('pageshow',settle)}
  if(this.context.addEventListener)this.context.addEventListener('statechange',settle);
  this.output.settle=settle;
 }
 /* An interrupted context does not start itself again: its clock stays on the instant it stopped,
  * so everything scheduled afterwards is queued against a frozen time and never sounds. Nor does a
  * media element fed by a MediaStream always come back from pause — the stream has to be handed to
  * it again. Both are asked for together, because either one alone leaves the room silent. */
 resumeOutput(){
  if(!this.context)return Promise.resolve();
  if(this.context.state==='running')return this.attachOutput();
  // One ask at a time: a statechange and a visibility change arriving together used to start two
  // attempts, and the element was handed the stream twice for nothing.
  this.resuming??=(async()=>{
   try{await this.context.resume()}catch(error){this.note('resume-refused',error?.message||'resume')}
   this.resuming=null;
   this.note('resume',this.context.state);
   if(this.context.state==='running')await this.attachOutput();
  })();
  return this.resuming;
 }
 attachOutput(){
  const output=this.output;
  if(!output)return Promise.resolve();
  try{output.element.srcObject=output.sink.stream;this.note('attach');return Promise.resolve(output.element.play()).catch(error=>{this.note('attach-refused',error?.message||'play')})}
  catch(error){this.note('attach-refused',error?.message||'attach');return Promise.resolve()}
 }
 get destination(){return this.output?.sink||this.master||this.context.destination}
 /* The direct path's own last stage: one gain before the context's destination, so this engine can still mute
  * a tab that is not the one sounding (#96) without an element to mute. */
 ensureMaster(){
  if(this.master||!this.context||typeof this.context.createGain!=='function')return this.master;
  try{this.master=this.context.createGain();this.master.gain.value=this.audible?1:0;this.master.connect(this.context.destination);this.note('output','context · '+(this.context.sampleRate||'?')+' Hz')}
  catch(error){this.master=null;this.note('output-failed',error?.message||'gain')}
  return this.master;
 }
 /* Where a job's sources connect: a gain of its own when the context has one, so cancelling fades it out
  * over a few milliseconds instead of stopping the sink's last input dead. */
 outlet(job){
  if(job.gain!==undefined)return job.gain||this.destination;
  job.gain=null;
  if(typeof this.context.createGain==='function'){try{job.gain=this.context.createGain();job.gain.connect(this.destination)}catch{job.gain=null}}
  return job.gain||this.destination;
 }
 /* The media element is handed its stream again: the same repair the page applies after an iOS
  * interruption. Used after a voice is cut mid-utterance, when the graph goes on and the element does not. */
 reattachOutput(reason){
  const output=this.output;
  if(!output)return Promise.resolve();
  this.note('reattach',reason);
  try{output.element.pause()}catch{}
  return this.attachOutput();
 }
 /* Stop a job's sources softly: gain to zero over 30 ms, sources stopped just after, the gain released later. */
 silence(job){
  const now=this.context?.currentTime||0,gain=job.gain;
  if(gain&&gain.gain&&typeof gain.gain.setValueAtTime==='function'){
   try{gain.gain.setValueAtTime(gain.gain.value,now);gain.gain.linearRampToValueAtTime(0,now+.03)}catch{}
   for(const source of job.sources){source.onended=null;try{source.stop(now+.04)}catch{try{source.stop()}catch{}}}
   setTimeout(()=>{try{gain.disconnect()}catch{}},200);
  }else for(const source of job.sources){source.onended=null;try{source.stop()}catch{}}
  job.sources.clear();
  if(job.playing)this.quiet('cut');
 }
 /* Seen on iPhone Safari (2026-09-19): a voice cut mid-utterance left the media element stuck on its last
  * instant while the graph went on, and the next utterance's first source unstuck it. So a cut is followed
  * by what the next utterance would do: a second and a half of silence into the same sink (half a second
  * still left the odd crackle on a phone, 2026-09-20). (A permanent silent
  * source was tried first and the phone's echo cancellation stopped covering the voice while it ran.) */
 /* The sink is never left without a source: iPhone Safari loops the last instant of an empty one, which is
  * heard as a crackle under a quiet room. Keeping that true is this engine's job and not something every
  * playback path has to remember: whatever stops making sound says so here, and the silence goes in only if
  * nobody else is still sounding. */
 quiet(reason){
  if(this.job?.sources?.size||this.presence)return false;
  this.tail(reason);
  return true;
 }
 tail(reason='end'){
  if(!this.context||typeof this.context.createBufferSource!=='function')return;
  try{
   const rate=this.context.sampleRate||48000,buffer=this.context.createBuffer(1,Math.round(rate*this.tailSeconds),rate);
   const source=this.context.createBufferSource();source.buffer=buffer;source.connect(this.destination);
   source.start(this.context.currentTime+.05);this.note('tail',reason);
  }catch(error){this.note('tail-failed',error?.message||reason)}
 }
 /* ----- the ambient bed: the conversation is working on the turn this browser sent (#42) -----
  * One loop, generated here, no asset: a band of noise between roughly 110 and 420 Hz mixed with two
  * quiet partials a fifth apart (220 and 330 Hz), the whole thing breathing once every two seconds.
  * The loop is four seconds long so both partials close a whole number of cycles at the seam, and the
  * noise is crossfaded into its own head so the join has no click.
  *
  * Its level is what keeps it out of the microphone's way: the buffer is normalized to peak 1 and the
  * gain *is* the peak amplitude, so 0.035 means -29 dBFS and nothing has to be guessed about it. It
  * leaves through the same media element as the voice (see ensureOutput), so a phone's echo
  * cancellation subtracts it instead of the detector hearing it as speech. */
 presenceBuffer(){
  /* A slow breath, not a beep. Two earlier attempts were rejected by ear: a filtered noise band read as a
   * fault, and a bright two-note pulse as a beep. This is one low tone with its fifth a whisper behind it,
   * swelling and fading once every few seconds, with the attack and release long enough that it never
   * sounds like an event — the point is company, not a signal. */
  const rate=this.context.sampleRate||48000,breath=this.presencePulseSeconds;
  const phrases=this.presencePhrases.map(([breaths,gap])=>({until:breaths*breath,seconds:breaths*breath+gap}));
  const length=Math.round(rate*phrases.reduce((total,phrase)=>total+phrase.seconds,0));
  const samples=new Float32Array(length);
  const partials=[[146.83,1],[220,.32],[293.66,.12]];   // D3, its fifth, its octave
  // The silences are zeros inside the same looping source, never a stopped one: the sink keeps rendering.
  for(let i=0,start=0,p=0;i<length;i++){
   const at=i/rate;
   if(at-start>=phrases[p].seconds&&p<phrases.length-1){start+=phrases[p].seconds;p++}
   const within=at-start;if(within>=phrases[p].until)continue;
   const phase=(within%breath)/breath;
   // Raised cosine over the whole breath: no attack to speak of, no release either, just a swell.
   const envelope=Math.pow(.5-.5*Math.cos(2*Math.PI*phase),1.6);
   let value=0;for(const [hz,weight] of partials)value+=weight*Math.sin(2*Math.PI*hz*at);
   samples[i]=envelope*value;
  }
  // The loop joins itself: the breath is at its quietest at both ends by construction.
  let peak=0;for(let i=0;i<length;i++)peak=Math.max(peak,Math.abs(samples[i]));
  if(peak>0)for(let i=0;i<length;i++)samples[i]/=peak;
  const buffer=this.context.createBuffer(1,length,rate);buffer.copyToChannel(samples,0);return buffer;
 }
 /* True once something audible has already left this output. A fresh sink must never be opened with the
  * bed: the greeting is what puts the media element in the phone's echo reference, and until it has
  * been through, a quiet loop is exactly the silent-first-source failure of 2026-09-19 again. */
 presenceReady(){return this.rendered||(!!this.greetedAt&&Date.now()-this.greetedAt>=this.greetSeconds*1000)}
 /* Returns whether the bed is now sounding. Refuses rather than queueing: a caller that cannot have it
  * yet asks again on the next turn. Never starts over speech — a job in flight owns the output. */
 startPresence({volume=.035,reason='working'}={}){
  if(this.presence)return true;
  if(!this.context||this.context.state!=='running'||this.job)return false;
  if(typeof this.context.createBufferSource!=='function'||typeof this.context.createGain!=='function')return false;
  const level=Math.min(this.presenceMaxVolume,Math.max(0,Number(volume)||0));
  if(!level)return false;
  if(!this.presenceReady()){this.note('presence-refused','output not yet rendering');return false}
  try{
   const gain=this.context.createGain(),source=this.context.createBufferSource();
   source.buffer=this.presenceBuffer();source.loop=true;source.connect(gain);gain.connect(this.destination);
   const now=this.context.currentTime,fade=this.presenceFadeSeconds;
   gain.gain.setValueAtTime(0,now);gain.gain.linearRampToValueAtTime(level,now+fade);
   source.start(now,this.presencePulseSeconds*this.presenceStartOffset);
   this.presence={source,gain,volume:level};this.note('presence-start',reason+' · '+level.toFixed(3));
   return true;
  }catch(error){this.note('presence-failed',error?.message||'presence');return false}
 }
 stopPresence(reason='stop'){
  const presence=this.presence;if(!presence)return false;
  this.presence=null;
  const now=this.context?.currentTime||0,fade=this.presenceFadeSeconds;
  try{
   presence.gain.gain.cancelScheduledValues?.(now);
   presence.gain.gain.setValueAtTime(presence.gain.gain.value,now);
   presence.gain.gain.linearRampToValueAtTime(0,now+fade);
  }catch{}
  presence.source.onended=null;
  try{presence.source.stop(now+fade+.02)}catch{try{presence.source.stop()}catch{}}
  setTimeout(()=>{try{presence.gain.disconnect()}catch{}},(fade+.3)*1000);
  this.note('presence-stop',reason);
  this.quiet('presence-stop');
  return true;
 }
 get supportsOutputSelection(){return typeof this.output?.element?.setSinkId==='function'||typeof (this.context||AudioContext.prototype).setSinkId==='function'}
 /* Experimental, per device (#59): keep the call going with the screen locked. iOS keeps a page running
  * while it is audible and freezes it a few seconds after it goes quiet, so pausing our own output on lock
  * is probably what ends the call. With this on, the element is paused only when the context really stops,
  * and the clock is watched as on the desktop. Off, each platform does what it always did. */
 keepPlayingWhileHidden(on){
  this.pauseWhileHidden=on?false:this.pausesByDefault;
  this.keepAliveWanted=!!on;
  this.note('locked-call',on?'on':'off');
  this.ensureKeepAlive();
  return !this.pauseWhileHidden;
 }
 /* The call with the screen locked, measured on 2026-09-26: iOS kept the page running while a reply
  * sounded, froze it within seconds of going quiet, and the room dropped the call until it woke. The
  * known way to stay "audible" is to never be silent: a faint noise floor, far below anything the
  * microphone's detector opens a turn on (-80 dBFS against a bed at -29), looping for as long as the
  * device asked for the locked call. Never the first thing into a fresh sink, like the bed. */
 ensureKeepAlive(){
  const wanted=!!this.keepAliveWanted&&!!this.context&&this.context.state==='running';
  if(!wanted){
   if(this.keepAlive){const source=this.keepAlive;this.keepAlive=null;try{source.stop()}catch{}this.note('keep-alive','stop')}
   return false;
  }
  if(this.keepAlive)return true;
  if(!this.presenceReady()||typeof this.context.createBufferSource!=='function')return false;
  try{
   const rate=this.context.sampleRate||48000,length=rate*2,samples=new Float32Array(length);
   for(let i=0;i<length;i++)samples[i]=(Math.random()*2-1)*this.keepAliveLevel;
   const buffer=this.context.createBuffer(1,length,rate);buffer.copyToChannel(samples,0);
   const source=this.context.createBufferSource();source.buffer=buffer;source.loop=true;source.connect(this.destination);
   source.start(this.context.currentTime);this.keepAlive=source;this.note('keep-alive','start');return true;
  }catch(error){this.note('keep-alive-failed',error?.message||'keep-alive');return false}
 }
 /* Whether this page is the one that sounds, when the same browser has the room open in several tabs (#96).
  * A tab that is not is muted, never paused: its playback, receipts and karaoke go on exactly as before, so
  * taking the sound back is instant and nothing it was playing is lost. */
 setAudible(on){
  this.audible=!!on;
  if(this.output?.element)this.output.element.muted=!this.audible;
  if(this.master)try{this.master.gain.value=this.audible?1:0}catch{}
  this.note('audible',this.audible?'on':'off');
  return this.audible;
 }
 async setOutputDevice(id){
  await this.unlock();
  if(!this.supportsOutputSelection)throw Error('Este navegador no permite elegir la salida de audio.');
  const selected=id||'default';
  if(typeof this.output?.element?.setSinkId==='function')await this.output.element.setSinkId(selected==='default'?'':selected);
  else await this.context.setSinkId(selected==='default'?'':selected);
  this.outputDeviceId=selected;return true;
 }
 stopProgress(job){
  if(job.raf!=null)globalThis.cancelAnimationFrame?.(job.raf);
  job.raf=null;
  if(job.onProgress){try{job.onProgress(null)}catch{}}
 }
 watchProgress(job){
  if(!job.onProgress||job.raf!=null||!globalThis.requestAnimationFrame)return;
  const tick=()=>{
   job.raf=null;if(this.job!==job)return;
   const now=this.context.currentTime;
   const cue=(job.cues||[]).find(c=>now>=c.start&&now<c.end)||null;
   if(cue!==job.lastCue){job.lastCue=cue;try{job.onProgress(cue)}catch{}}
   if(this.job===job)job.raf=globalThis.requestAnimationFrame(tick);
  };
  tick();
 }
 /* Playout is watched on the wall clock: audio time that stops advancing while an utterance plays is an
  * interrupted context or a starved media element, which on iOS sounds like the last instant on a loop.
  * The output is asked back once; a clock that stays frozen fails the utterance, so the room shows it. */
 watchClock(job){
  if(job.clock!=null||!this.context)return;
  const wall=()=>Date.now();
  job.clock={wallAt:wall(),timeAt:this.context.currentTime,stalls:0};
  const check=()=>{
   if(this.job!==job)return;
   const now=wall(),elapsed=now-job.clock.wallAt,advanced=this.context.currentTime-job.clock.timeAt;
   // A page that went away is not a stuck device: the element is paused on purpose while the page is hidden
   // (see the settle handler), so nothing can advance. Counting it as a stall declared perfectly good replies
   // failed when the phone's screen locked mid-utterance (2026-09-20, read from the room's audio reports).
   // A background tab whose clock still moves is playing, not away: it is watched like a visible one (#96).
   if(typeof document!=='undefined'&&document.hidden&&(this.pauseWhileHidden||advanced<.05)){
    if(!job.clock.away){job.clock.away=now;this.note('clock-away',this.context.state)}
    // A page that never comes back must not leave the room waiting for a receipt that will never arrive:
    // the room holds everything else behind this utterance (2026-09-20, two replies stuck in a queue).
    else if(now-job.clock.away>=this.awayLimitMs){
     this.fail(Error('La página estuvo en segundo plano mientras sonaba; la locución se repite al volver.'));return;
    }
    job.clock.wallAt=now;job.clock.timeAt=this.context.currentTime;
    job.clock.timer=setTimeout(check,this.stallCheckMs);return;
   }
   job.clock.away=0;
   if(elapsed>=this.stallAfterMs){
    if(advanced<.05){
     job.clock.stalls++;this.stalls++;
     this.note('stall',this.context.state+' · '+(this.output?.element?.paused?'element paused':'element playing')+' · '+job.clock.stalls);
     if(job.clock.stalls>=this.stallLimit){this.fail(Error('La reproducción se detuvo en este dispositivo; la locución se da por fallida.'));return}
     if(this.output?.element){try{this.output.element.pause()}catch{}}
     this.resumeOutput();
    }else job.clock.stalls=0;
    job.clock.wallAt=now;job.clock.timeAt=this.context.currentTime;
   }
   job.clock.timer=setTimeout(check,this.stallCheckMs);
  };
  job.clock.timer=setTimeout(check,this.stallCheckMs);
 }
 stopClock(job){if(job.clock?.timer)clearTimeout(job.clock.timer);if(job.clock)job.clock.timer=null}
 cancel(){this.announce('','hidden');this.stopPresence('speech');const job=this.job;this.job=null;this.worker?.postMessage({type:'cancel'});if(!job)return;this.note('cancel',job.playing?'playing':'pending');this.stopProgress(job);this.stopClock(job);clearTimeout(job.timer);this.silence(job);
  // Seen on iPhone Safari (2026-09-19): after a voice is cut mid-utterance the graph keeps running and
  // reporting playback, while what leaves the element is stuck. The element gets its stream back once
  // the fade is over; a cancel while the context is stopped pauses it at once so it cannot loop.
  if(this.output?.element&&this.context?.state!=='running'){try{this.output.element.pause()}catch{}}
  job.reject(new DOMException('Audio cancelado','AbortError'))}
 ensure(device){if(this.worker&&this.device===device)return;this.worker?.terminate();this.ready=false;this.device=device;this.worker=new Worker('/voice-browser/worker.js?v='+encodeURIComponent(globalThis.sidevoiceBuildId||'dev'),{type:'module'});this.worker.onmessage=({data})=>this.receive(data);this.worker.onerror=e=>this.fail(Error(e.message||'No se pudo iniciar el motor de voz'))}
 fail(error){this.announce(error.message,this.ready?'inline':'error');this.ready=false;const job=this.job;this.note('fail',error?.message||'error');if(!job)return;this.job=null;this.stopProgress(job);this.stopClock(job);clearTimeout(job.timer);this.silence(job);this.worker?.terminate();this.worker=null;job.reject(error)}
 receive(d){const job=this.job;if(!job||d.id!==job.id)return;
  clearTimeout(job.timer);job.timer=setTimeout(()=>this.fail(Error('El modelo tardó demasiado. Vuelve a prepararlo.')),180000);
  if(d.type==='progress'){const p=d.progress;const text=p.status==='voice'?'Cargando la voz seleccionada…':p.status==='generating'?'Preparando el primer audio…':'Cargando modelo'+(p.file?' · '+p.file:'')+(p.progress!=null?' · '+Math.round(p.progress)+'%':'');job.status(text);if(!this.ready&&!job.playing)this.announce(text,'loading',p.progress??null)}
  if(d.type==='fallback'){job.status('GPU no disponible · Preparando CPU');this.announce('GPU no disponible · Preparando CPU')}
  if(d.type==='ready'){this.ready=true;this.announce('','hidden');job.status('Modelo listo · '+(d.device==='webgpu'?'GPU':'CPU'));if(job.load)this.complete(job)}
  if(d.type==='error')this.fail(Error(d.error));
  if(d.type==='audio'){this.announce('','hidden');if(this.context.state!=='running'){this.note('audio-while-stopped',this.context.state);this.resumeOutput()}
   this.watchRate(d.sampleRate);
   const buffer=this.context.createBuffer(1,d.samples.length,d.sampleRate);buffer.copyToChannel(d.samples,0);
   const source=this.context.createBufferSource();source.buffer=buffer;source.connect(this.outlet(job));
   const start=Math.max(this.context.currentTime+.03,job.end);job.end=start+buffer.duration;job.sources.add(source);
   source.onended=()=>{job.sources.delete(source);if(this.job===job&&job.done&&!job.sources.size)this.complete(job)};
   const range=chunkTextRange(job.text,d.text,job.textCursor);
   if(range){job.textCursor=range.to;job.cues.push({...range,start,end:start+buffer.duration,mode:'chunk'})}
   source.start(start);this.watchProgress(job);if(!job.playing){job.playing=true;job.onPlaying()}this.watchClock(job);
   job.status('Voz en tu navegador · '+(d.device==='webgpu'?'GPU':'CPU'));
  }
  if(d.type==='done'){clearTimeout(job.timer);job.done=true;if(!job.sources.size)this.complete(job)}
 }
 /* An utterance that ends on its own leaves the sink as empty as a cut one does, and on iPhone Safari an
  * empty sink loops its last instant — the crackle heard under the room while nobody speaks (2026-09-20).
  * So the end of the voice gets the same silent tail the cut has had since 2026-09-19. */
 complete(job){if(this.job!==job)return;this.stopProgress(job);this.stopClock(job);this.note('complete');this.announce('','hidden');clearTimeout(job.timer);this.job=null;if(job.playing)this.quiet('complete');if(job.gain){const gain=job.gain;setTimeout(()=>{try{gain.disconnect()}catch{}},200)}job.resolve()}
 run(type,options={},status=()=>{},onPlaying=()=>{},onProgress){
  this.cancel();const device=options.device||'auto';this.ensure(device);const message=type==='load'?'Cargando modelo…':'Preparando voz…';status(message);if(!this.ready)this.announce(message);
  return new Promise((resolve,reject)=>{const id=++this.serial;this.job={id,resolve,reject,status,onPlaying,onProgress,text:options.text||'',textCursor:0,cues:[],load:type==='load',sources:new Set(),end:0,done:false};this.job.timer=setTimeout(()=>this.fail(Error('No se pudo preparar el modelo a tiempo.')),180000);this.worker.postMessage({type,id,...options,device})})
 }
 playEncoded({audio_base64,text='',alignment},status=()=>{},onPlaying=()=>{},onProgress){
  this.cancel();
  return new Promise((resolve,reject)=>{
   const job=this.job={id:++this.serial,resolve,reject,status,onPlaying,onProgress,cues:[],sources:new Set(),done:true};
   job.timer=setTimeout(()=>{if(this.job===job)this.fail(Error('No se pudo preparar el audio a tiempo.'))},30000);
   (async()=>{
    await this.unlock();
    if(this.job!==job)return;
    const bytes=Uint8Array.from(atob(audio_base64),c=>c.charCodeAt(0));
    const buffer=await this.context.decodeAudioData(bytes.buffer);
    if(this.job!==job)return;
    clearTimeout(job.timer);
    this.watchRate(buffer.sampleRate);
    const source=this.context.createBufferSource();source.buffer=buffer;source.connect(this.outlet(job));job.sources.add(source);
    source.onended=()=>{job.sources.delete(source);if(this.job===job)this.complete(job)};
    const start=this.context.currentTime;
    const aligned=alignedWordCues(text,alignment,buffer.duration);
    job.cues=(aligned.length?aligned:(text?[{from:0,to:text.length,start:0,end:buffer.duration,mode:'utterance'}]:[]))
     .map(cue=>({...cue,start:cue.start+start,end:cue.end+start}));
    source.start(start);job.playing=true;onPlaying();this.note('play-encoded',this.context.state);this.watchProgress(job);this.watchClock(job);status('Reproduciendo voz de ElevenLabs');
   })().catch(error=>{if(this.job===job)this.fail(error)});
  });
 }
 prepare(options,status){return this.run('load',options,status)}
 speak(options,status,onPlaying,onProgress){return this.run('speak',options,status,onPlaying,onProgress)}
}
window.roomVoice=new RoomVoice();
