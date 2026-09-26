import assert from 'node:assert/strict';
import test from 'node:test';
import { detectAgentActivity } from './activity-detector.ts';

test('detects Codex working and completed states by the latest cue', () => {
  assert.equal(detectAgentActivity('codex', '• Working (12s • esc to interrupt)'), 'working');
  assert.equal(detectAgentActivity('codex', '• Waiting…'), 'working');
  assert.equal(detectAgentActivity('claude', '❤ Waiting for Claude to respond…'), 'working');
  assert.equal(detectAgentActivity('codex', 'esc to interrupt\nWorked for 24s'), 'idle');
  assert.equal(detectAgentActivity('codex', 'Worked for 24s\n⠙ Working (1s)'), 'working');
});

test('detects Codex and Claude approval prompts', () => {
  assert.equal(detectAgentActivity('codex', [
    'Would you like to run the following command?',
    '› 1. Yes, proceed',
    '  2. Yes, and don\'t ask again',
    '  3. No, and tell Codex what to do differently',
  ].join('\n')), 'waiting');
  assert.equal(detectAgentActivity('claude', [
    'Do you want to proceed?',
    '❯ 1. Yes',
    '  2. Yes, and don\'t ask again',
    '  3. No',
    'Esc to cancel · Tab to amend',
  ].join('\n')), 'waiting');
  assert.equal(detectAgentActivity('claude', 'Enter to select · ↑/↓ to navigate · Esc to cancel'), 'waiting');
  assert.equal(detectAgentActivity('codex', 'Waiting for your approval'), 'waiting');
});

test('does not treat quoted approval language or silence as a transition', () => {
  assert.equal(detectAgentActivity('claude', 'The docs say “Do you want to proceed?” in this example.'), undefined);
  assert.equal(detectAgentActivity('claude', 'Do you want to proceed?\nThe answer is yes.'), undefined);
  assert.equal(detectAgentActivity('codex', ''), undefined);
});

test('detects Claude completion and lets a later busy cue win', () => {
  assert.equal(detectAgentActivity('claude', '✻ Worked for 31s'), 'idle');
  assert.equal(detectAgentActivity('claude', '✻ Baked for 1m\n✽ Thinking…'), 'working');
  assert.equal(detectAgentActivity('claude', 'I waited for 2 minutes while testing.'), undefined);
});
