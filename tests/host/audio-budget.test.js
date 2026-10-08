import {test} from 'node:test';
import assert from 'node:assert/strict';
import {audioDecodedBudget,validateDecodedAudio} from '../../crates/nir-platform-web/host.js';
const descriptor={duration_us:'8000000',decoded_bytes:768000};
test('Web PCM capacity matches integer Rust policy and admits actual resampled mono/stereo',()=>{
    assert.equal(audioDecodedBudget(descriptor,48000),3072008);
    assert.equal(validateDecodedAudio(descriptor,{length:384000,numberOfChannels:1,sampleRate:48000},48000),1536000);
    assert.equal(validateDecodedAudio(descriptor,{length:384001,numberOfChannels:2,sampleRate:48000},48000),3072008);
    assert.equal(audioDecodedBudget({duration_us:'1',decoded_bytes:4},48000),16);
    assert.equal(audioDecodedBudget({duration_us:'22',decoded_bytes:4},48000),24);
    assert.equal(audioDecodedBudget({duration_us:'1000000',decoded_bytes:768000},24000),768000);
});
test('overflow, wrong rate, excess channels and payload outside admitted capacity fail explicitly',()=>{
    for(const rate of [0,-1,NaN,48000.5,2**32])assert.throws(()=>audioDecodedBudget(descriptor,rate),e=>e.code==='E_AUDIO_RATE');
    assert.throws(()=>audioDecodedBudget({duration_us:'18446744073709551615',decoded_bytes:4},2**32-1),e=>e.code==='E_AUDIO_MEMORY');
    for(const shape of [
        {length:384002,numberOfChannels:2,sampleRate:48000},
        {length:384000,numberOfChannels:3,sampleRate:48000},
        {length:384000,numberOfChannels:1,sampleRate:44100},
        {length:0,numberOfChannels:1,sampleRate:48000},
    ])assert.throws(()=>validateDecodedAudio(descriptor,shape,48000),e=>e.code==='E_AUDIO_MEMORY');
});

test('decoded time must match authored time before scene readiness, allowing one resampled frame',()=>{
    for(const length of [383999,384000,384001])
        assert.equal(validateDecodedAudio(descriptor,{length,numberOfChannels:1,sampleRate:48000},48000),length*4);
    for(const length of [1,383998,384002,768000])
        assert.throws(()=>validateDecodedAudio(descriptor,{length,numberOfChannels:1,sampleRate:48000},48000),e=>e.code==='E_AUDIO_DURATION');
    const fractional={duration_us:'124001',decoded_bytes:11904};
    for(const length of [5467,5468,5469])
        assert.equal(validateDecodedAudio(fractional,{length,numberOfChannels:1,sampleRate:44100},44100),length*4);
    assert.throws(()=>validateDecodedAudio(fractional,{length:5466,numberOfChannels:1,sampleRate:44100},44100),e=>e.code==='E_AUDIO_DURATION');
});
