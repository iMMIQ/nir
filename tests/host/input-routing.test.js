import {test} from 'node:test';
import assert from 'node:assert/strict';
import {pointerAction,primaryKeyAction} from '../../crates/nir-platform-web/host.js';

test('secondary and middle pointers never activate story or title hits',()=>{
    assert.deepEqual(pointerAction(0,'Story',{type:'advance'}),{type:'advance'});
    assert.deepEqual(pointerAction(2,'Story',{type:'advance'}),{type:'menu'});
    assert.equal(pointerAction(2,'Title',{type:'new_game'}),null);
    assert.equal(pointerAction(1,'Title',{type:'new_game'}),null);
    assert.deepEqual(pointerAction(2,'Settings',null),{type:'close'});
});
test('unfocused primary key respects available menu actions',()=>{
    assert.equal(primaryKeyAction('Title',false,[{enabled:false,action:{type:'image_menu_entry',function:'replay'}}]),null);
    assert.equal(primaryKeyAction('Title',false,[{enabled:false,action:{type:'new_game'}}]),null);
    assert.deepEqual(primaryKeyAction('Title',false,[{enabled:true,action:{type:'new_game'}}]),{type:'new_game'});
    assert.deepEqual(primaryKeyAction('Story',false,[]),{type:'advance'});
    assert.deepEqual(primaryKeyAction('Story',true,[]),{type:'continue'});
    assert.equal(primaryKeyAction('Menu',true,[]),null);
});
