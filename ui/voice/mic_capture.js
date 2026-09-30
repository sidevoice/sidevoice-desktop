/* Microphone capture for the room page. Runs inside an AudioWorklet: takes the mono float input
 * at the context rate, resamples it to the rate the room declared and posts 20 ms chunks of
 * 16-bit PCM to the page, which forwards each one as a binary WebSocket frame.
 * The pure parts are also loaded by test_mic_capture.cjs outside any audio context. */
class Resampler{
 // Linear interpolation with carry-over across blocks, so block boundaries never click.
 constructor(from,to){this.step=from/to;this.position=0;this.last=0}
 process(input){
  const n=input.length;if(this.step===1||!n)return input;
  const out=[];let p=this.position;
  while(p<n-1){const i=Math.floor(p),a=i<0?this.last:input[i],b=input[i+1];out.push(a+(b-a)*(p-i));p+=this.step}
  this.position=p-n;this.last=input[n-1];
  return Float32Array.from(out);
 }
}
function toInt16(samples){const out=new Int16Array(samples.length);for(let i=0;i<samples.length;i++){const s=Math.max(-1,Math.min(1,samples[i]));out[i]=s<0?s*32768:s*32767}return out}
class MicCapture extends AudioWorkletProcessor{
 constructor(options){super();const target=options.processorOptions.sampleRate;this.resampler=new Resampler(sampleRate,target);this.size=Math.round(target/50);this.chunk=new Int16Array(this.size);this.filled=0}
 process(inputs){
  const channel=inputs[0]&&inputs[0][0];if(!channel)return true;
  const samples=toInt16(this.resampler.process(channel));
  // Posting transfers the buffer, which detaches this.chunk: its length then reads 0,
  // so the next chunk is sized from this.size and never from the detached one.
  for(let i=0;i<samples.length;i++){this.chunk[this.filled++]=samples[i];if(this.filled===this.size){this.port.postMessage(this.chunk.buffer,[this.chunk.buffer]);this.chunk=new Int16Array(this.size);this.filled=0}}
  return true;
 }
}
registerProcessor('mic-capture',MicCapture);
