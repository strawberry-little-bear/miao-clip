const test = require('node:test');
const assert = require('node:assert/strict');
const { isCaptureSuspended } = require('../ui/capture-state.js');

test('paused capture blocks automatic clipboard reads', () => {
  assert.equal(isCaptureSuspended({ paused: true }), true);
  assert.equal(isCaptureSuspended({ paused: false, privacy_mode: false }), false);
});

test('privacy mode blocks automatic clipboard reads in both API naming styles', () => {
  assert.equal(isCaptureSuspended({ privacyMode: true }), true);
  assert.equal(isCaptureSuspended({ privacy_mode: true }), true);
  assert.equal(isCaptureSuspended({ paused: false, privacyMode: false }), false);
});

test('missing capture state defaults to active listening', () => {
  assert.equal(isCaptureSuspended(undefined), false);
  assert.equal(isCaptureSuspended(null), false);
});
