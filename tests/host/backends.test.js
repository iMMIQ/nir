import {test} from 'node:test';
import assert from 'node:assert/strict';
import {initializeBackend} from '../../crates/nir-platform-web/host.js';

test('auto replaces a bound canvas before WebGL2 retry',async()=>{
    const calls=[];
    const result=await initializeBackend({probe:async()=> 'webgpu',replaceCanvas:()=>calls.push('replace'),create:async backend=>{
        calls.push(backend);if(backend==='webgpu')throw Error('device initialization failed');return {backend};
    }});
    assert.deepEqual(calls,['webgpu','replace','webgl2']);
    assert.equal(result.engine.backend,'webgl2');
    assert.match(result.fallbackReason,/device initialization/);
});

test('forced backend errors do not silently switch backend',async()=>{
    const calls=[];
    await assert.rejects(initializeBackend({requested:'webgpu',probe:()=>assert.fail(),replaceCanvas:()=>assert.fail(),create:async backend=>{
        calls.push(backend);throw Error('GPU unavailable');
    }}),/GPU unavailable/);
    assert.deepEqual(calls,['webgpu']);
});

test('failed capability probe uses a fresh WebGL2 initialization',async()=>{
    const result=await initializeBackend({probe:async()=>{throw Error('adapter unavailable');},replaceCanvas:()=>assert.fail(),create:async backend=>({backend})});
    assert.equal(result.engine.backend,'webgl2');
    assert.match(result.fallbackReason,/adapter unavailable/);
});
