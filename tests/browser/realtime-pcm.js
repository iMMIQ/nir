export const realtimePcmProcessor = `class Capture extends AudioWorkletProcessor {
      constructor(options) {
        super(); this.limit=options.processorOptions.frames;
        this.channels=2;this.pcm=new Float32Array(this.limit*this.channels); this.count=0; this.first=null;
        this.blocks=0; this.discontinuities=0; this.previousEnd=null;
        this.port.onmessage=event=>{if(event.data==='arm')this.armed=true;};
      }
      process(inputs,outputs) {
        // A silent output keeps the tap in the live graph without changing
        // the player's existing audible route.
        for(const channel of outputs[0]||[])channel.fill(0);
        if(!this.armed||!this.pcm)return true;
        const input=inputs[0]||[],frames=outputs[0][0].length;
        if(this.first===null){this.first=currentFrame;this.quantumFrames=frames;}
        if(this.previousEnd!==null&&this.previousEnd!==currentFrame)this.discontinuities++;
        this.previousEnd=currentFrame+frames; this.blocks++;
        const n=Math.min(frames,this.limit-this.count);
        for(let i=0;i<n;i++)for(let c=0;c<this.channels;c++)
          this.pcm[(this.count+i)*this.channels+c]=input[c]?.[i]??0;
        this.count+=n;
        if(this.count===this.limit) {
          this.port.postMessage({pcm:this.pcm.buffer,first:this.first,frames:this.count,
            blocks:this.blocks,discontinuities:this.discontinuities,quantumFrames:this.quantumFrames,
            rate:sampleRate,channels:this.channels},[this.pcm.buffer]);
          this.pcm=null; this.armed=false;
        }
        return true;
      }
    } registerProcessor('nir-test-pcm',Capture);`;

// Test-only render-thread tap. The player's original destination connection is
// retained. The parallel consumer writes zeroes, so it cannot double the sound.
// PCM storage is bounded, and transferred only once: a blocked JS main thread
// cannot lose a message or fabricate a silent interval in this measurement.
export async function installRealtimePcm(page, {rate, seconds=24}) {
  await page.addInitScript(({rate,seconds}) => {
    const moduleUrl='/__nir_test_pcm.js';
    const Native=AudioContext,contexts=new WeakMap();
    const audit=globalThis.realtimePcm={contexts:[],starts:[],stops:[],destinationConnections:0};
    globalThis.AudioContext=class extends Native {
      constructor(options={}) {
        super({...options,sampleRate:rate});
        const index=audit.contexts.length;contexts.set(this,index);audit.contexts.push(this);
        if(index===0) audit.ready=this.audioWorklet.addModule(moduleUrl).then(()=>{
          const tap=new AudioWorkletNode(this,'nir-test-pcm',{
            numberOfInputs:1,numberOfOutputs:1,outputChannelCount:[2],
            channelCount:2,channelCountMode:'explicit',processorOptions:{frames:Math.round(rate*seconds)}});
          audit.tap=tap;tap.connect(this.destination);
          audit.finished=new Promise((resolve,reject)=>{
            tap.port.onmessage=event=>{audit.capture=event.data;resolve();};
            tap.onprocessorerror=()=>reject(new Error('PCM AudioWorklet processor failed'));
          });
        });
      }
    };
    const connect=AudioNode.prototype.connect;
    AudioNode.prototype.connect=function(target,...rest) {
      const result=connect.call(this,target,...rest);
      if(contexts.get(this.context)===0&&target===this.context.destination&&this!==audit.tap) {
        if(!audit.tap)throw new Error('PCM tap not ready before destination connection');
        audit.destinationConnections++;audit.finalGain=this;
        connect.call(this,audit.tap);
      }
      return result;
    };
    const create=Native.prototype.createBufferSource;
    Native.prototype.createBufferSource=function(...args) {
      const source=create.apply(this,args),start=source.start,stop=source.stop;
      source.start=function(...args) {
        if(contexts.get(this.context)===0) {
          if(audit.starts.length===0) {
            if(this.buffer.numberOfChannels!==2)throw new Error('PCM fixture must decode to stereo');
            audit.reference=new Float32Array(this.buffer.length*2);
            for(let c=0;c<2;c++){
              const samples=this.buffer.getChannelData(c);
              for(let i=0;i<samples.length;i++)audit.reference[i*2+c]=samples[i];
            }
            audit.firstSource=this;
          }
          audit.starts.push({clock:this.context.currentTime,when:args[0]||0,offset:args[1]||0,
            frames:this.buffer.length,channels:this.buffer.numberOfChannels,rate:this.buffer.sampleRate,loop:this.loop,
            loopStart:this.loopStart,loopEnd:this.loopEnd,gain:audit.finalGain?.gain.value});
        }
        return start.apply(this,args);
      };
      source.stop=function(...args) {
        if(contexts.get(this.context)===0)audit.stops.push({clock:this.context.currentTime});
        return stop.apply(this,args);
      };
      return source;
    };
  }, {rate,seconds});
}

export async function armRealtimePcm(page) {
  await page.evaluate(async()=>{await realtimePcm.ready;realtimePcm.tap.port.postMessage('arm');});
}

export async function readRealtimePcm(page) {
  return page.evaluate(async()=>{
    await realtimePcm.finished;
    const encode=bytes=>{
      let binary='';for(let i=0;i<bytes.length;i+=8192)binary+=String.fromCharCode(...bytes.subarray(i,i+8192));
      return btoa(binary);
    };
    const {pcm,...meta}=realtimePcm.capture;
    return {meta,pcm:encode(new Uint8Array(pcm)),
      reference:encode(new Uint8Array(realtimePcm.reference.buffer)),
      starts:realtimePcm.starts,stops:realtimePcm.stops,
      destinationConnections:realtimePcm.destinationConnections};
  });
}

export function floatPcm(base64) {
  const bytes=Buffer.from(base64,'base64');
  return new Float32Array(bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.byteLength));
}

// Independent sample-index oracle; it never imports the production loop mapper.
// Find the initial render quantum's scheduling offset using the distinctive
// intro, then compare every subsequent sample, including all loop seams.
export function compareRealtimePcm(capture) {
  const pcm=floatPcm(capture.pcm),reference=floatPcm(capture.reference),start=capture.starts[0];
  const channels=capture.meta.channels;
  if(channels!==2||start.channels!==channels||reference.length!==start.frames*channels||pcm.length!==capture.meta.frames*channels)
    throw new Error('Stereo PCM shape mismatch');
  const end=start.loopEnd?Math.round(start.loopEnd*start.rate):start.frames;
  const begin=Math.round(start.loopStart*start.rate),body=end-begin;
  const index=i=>i<end?i:begin+(i-end)%body;
  const gain=start.gain;
  const zeroErrorAt=offset=>{
    let sum=0;for(let i=0;i<1024;i++)for(let c=0;c<channels;c++)sum+=(pcm[(offset+i)*channels+c]-reference[i*channels+c]*gain)**2;
    return sum/(1024*channels);
  };
  let offset=0,best=Infinity;
  // Native start(0) takes effect at a render boundary. The tap can already
  // have captured silence while the player prepares its first scene.
  // Bound alignment to the actual native start clock. Searching an entire
  // track could accidentally hide a dropped first cycle of a whole loop.
  const scheduledFrame=Math.round(start.clock*start.rate)-capture.meta.first;
  const allowance=2*capture.meta.quantumFrames;
  const limit=Math.min(capture.meta.frames-1024,scheduledFrame+allowance);
  for(let i=Math.max(0,scheduledFrame-allowance);i<=limit;i++) {
    const error=zeroErrorAt(i);if(error<best){best=error;offset=i;}
  }
  let maxError=0,squared=0,badFrames=0,seams=0,maxSeamError=0,firstBad=null;
  const badFramesByChannel=Array(channels).fill(0),firstBadByChannel=Array(channels).fill(null);
  const tolerance=0.000002;
  for(let i=offset;i<capture.meta.frames;i++) {
    const relative=i-offset,j=index(relative);let bad=false;
    const seam=relative>=end&&(relative-end)%body===0;
    if(seam)seams++;
    for(let c=0;c<channels;c++){
      const error=Math.abs(pcm[i*channels+c]-reference[j*channels+c]*gain);
      maxError=Math.max(maxError,error);squared+=error*error;
      if(error>tolerance){bad=true;badFramesByChannel[c]++;firstBadByChannel[c]??=i;}
      if(seam)maxSeamError=Math.max(maxSeamError,error);
    }
    if(bad){badFrames++;firstBad??=i;}
  }
  const frames=capture.meta.frames-offset;
  return {offset,scheduledFrame,onsetDeltaFrames:offset-scheduledFrame,
    bestAlignmentMse:best,firstFrame:capture.meta.first+offset,
    frames,channels,seams,maxError,rmsError:Math.sqrt(squared/(frames*channels)),badFrames,firstBad,badFramesByChannel,firstBadByChannel,
    maxSeamError,rate:start.rate,loopStartFrame:begin,loopEndFrame:end,bodyFrames:body,
    authoredSeamJump:Math.max(...Array.from({length:channels},(_,c)=>Math.abs(reference[(end-1)*channels+c]-reference[begin*channels+c])))*gain,
    tolerance,scope:'actual Web Audio render graph before device output; not DAC, speaker, or Android'};
}
