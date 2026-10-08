import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mediaMemorySnapshot} from '../../crates/nir-platform-web/host.js';

test('audio payload accounting uses decoded frames and channels, deduplicates aliases and retains live sources',()=>{
    const mono={length:48000,numberOfChannels:1},stereo={length:44100,numberOfChannels:2};
    const bytes=new ArrayBuffer(2000),encoded=new Map([['a',bytes],['alias',bytes]]);
    const cached=new Map([['mono',mono],['alias',mono],['stereo',stereo]]);
    const voices=new Map([['one',{source:{buffer:mono}}],['two',{source:{buffer:mono}}]]);
    let s=mediaMemorySnapshot(encoded,cached,voices);
    assert.equal(s.encoded_cache_bytes,2000);assert.equal(s.encoded_cache_objects,1);
    assert.equal(s.decoded_audio_bytes,48000*4+44100*2*4);assert.equal(s.decoded_audio_buffers,2);
    assert.equal(s.cached_audio_assets,3);assert.equal(s.active_audio_bytes,48000*4);assert.equal(s.active_audio_sources,2);
    cached.clear();s=mediaMemorySnapshot(encoded,cached,voices);
    assert.equal(s.decoded_audio_bytes,48000*4);assert.equal(s.cached_audio_bytes,0);
    voices.clear();assert.equal(mediaMemorySnapshot(encoded,cached,voices).decoded_audio_bytes,0);
});

test('transferred image staging disappears from host ownership and output does not mutate input',()=>{
    const pixels=new ArrayBuffer(320*180*4),staging=new Map([['pending',pixels]]);
    const args=[new Map(),new Map(),new Map(),staging];
    assert.equal(mediaMemorySnapshot(...args).image_staging_bytes,320*180*4);
    const transferred=structuredClone(pixels,{transfer:[pixels]});
    assert.equal(transferred.byteLength,320*180*4);
    assert.equal(mediaMemorySnapshot(...args).image_staging_bytes,0);
    assert.equal(staging.size,1);
});
