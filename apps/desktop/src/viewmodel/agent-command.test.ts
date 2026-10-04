import assert from 'node:assert/strict';
import test from 'node:test';
import { detectAgentLaunch } from './agent-command.ts';

test('detects agent launches and model flags', () => {
  assert.deepEqual(detectAgentLaunch('codex -m gpt-6.1-sol'), { cli: 'codex', requestedModel: 'gpt-6.1-sol' });
  assert.deepEqual(detectAgentLaunch('command claude --model="claude sonnet"'), { cli: 'claude', requestedModel: 'claude sonnet' });
  assert.deepEqual(detectAgentLaunch("gemini --model 'gemini-pro'"), { cli: 'gemini', requestedModel: 'gemini-pro' });
  assert.deepEqual(detectAgentLaunch('claude --continue'), { cli: 'claude', requestedModel: undefined });
  assert.equal(detectAgentLaunch('echo codex --model nope'), null);
});
