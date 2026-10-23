import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import type { SessionRecord } from '../domain/session-record';
import { taskAgentCommand, taskExecutionPrompt } from '../domain/task-execution.ts';
import { executeSessionTasks, type TaskExecutionStore } from './task-execution.ts';

const task = (id: string): SessionRecord => ({
  id, profileId: 'profile-1', kind: 'task', status: 'in progress',
  title: `Task ${id}`, content: '한국어\n  code\n\tEnglish\r\n"quotes" $()`', createdAt: 1, updatedAt: 2,
});

function store(tasks: SessionRecord[] = [task('one'), task('two')]) {
  const calls: { profileId: string; ids?: string[] }[] = [];
  const restored: SessionRecord[][] = [];
  const repository: TaskExecutionStore = {
    claimTodoTasks: async (profileId, ids) => {
      calls.push({ profileId, ids });
      return tasks.filter((value) => !ids || ids.includes(value.id));
    },
    restoreClaimedTasks: async (records) => { restored.push(records); },
  };
  return { repository, calls, restored };
}

test('individual play hands off only that task, with its ID and full requirements', async () => {
  const f = store();
  const sent: string[] = [];
  await executeSessionTasks(f.repository, 'profile-1', ['two'], async (prompt) => { sent.push(prompt); });
  assert.deepEqual(f.calls, [{ profileId: 'profile-1', ids: ['two'] }]);
  assert.equal(sent.length, 1);
  assert.ok(sent[0].includes(JSON.stringify({ profileId: 'profile-1', tasks: [{ id: 'two', title: 'Task two', content: task('two').content }] })));
  assert.ok(!sent[0].includes('Task one'));
  assert.deepEqual(f.restored, []);
});

test('run all claims current todos and submits them together once in stored order', async () => {
  const f = store();
  const sent: string[] = [];
  await executeSessionTasks(f.repository, 'profile-1', undefined, async (prompt) => { sent.push(prompt); });
  assert.equal(f.calls[0].ids, undefined);
  assert.equal(sent.length, 1);
  assert.ok(sent[0].indexOf('Task one') < sent[0].indexOf('Task two'));
  assert.match(sent[0], /done.*only after completing and verifying/);
  assert.deepEqual(f.restored, []);
});

test('no remaining todo or failed claim never sends input', async () => {
  let sends = 0;
  const f = store([]);
  await assert.rejects(executeSessionTasks(f.repository, 'profile-1', undefined, async () => { sends++; }), /no to-do tasks/);
  f.repository.claimTodoTasks = async () => { throw new Error('storage failed'); };
  await assert.rejects(executeSessionTasks(f.repository, 'profile-1', undefined, async () => { sends++; }), /storage failed/);
  assert.equal(sends, 0);
});

test('failed terminal handoff restores exactly the claimed records', async () => {
  const f = store();
  await assert.rejects(executeSessionTasks(f.repository, 'profile-1', ['two'], async () => { throw new Error('disconnected'); }), /disconnected/);
  assert.deepEqual(f.restored, [[task('two')]]);
});

test('rollback failures remain visible instead of claiming the task is runnable again', async () => {
  const f = store();
  f.repository.restoreClaimedTasks = async () => { throw new Error('disk full'); };
  await assert.rejects(executeSessionTasks(f.repository, 'profile-1', undefined, async () => { throw new Error('send failed'); }), /send failed.*status could not be restored.*disk full/);
});

test('a mismatched profile cannot be dispatched and is restored', async () => {
  const f = store();
  let sends = 0;
  await assert.rejects(executeSessionTasks(f.repository, 'other', undefined, async () => { sends++; }), /this session/);
  assert.equal(sends, 0);
  assert.equal(f.restored.length, 1);
});

test('task content keeps its exact formatting in JSON without injecting terminal control keys', () => {
  const value = task('one');
  value.content += '\x1b[201~\x03\x00\x7f';
  const prompt = taskExecutionPrompt('profile-1', [value]);
  assert.doesNotMatch(prompt, /[\x00-\x1f\x7f]/);
  const payload = JSON.parse(prompt.slice(prompt.indexOf('{"profileId":')));
  assert.equal(payload.tasks[0].content, value.content);
});

test('initial prompts and models are literal shell arguments for each CLI', () => {
  const prompt = 'Execute tasks "quoted" \'literal\' $(printf injected) `printf injected`; # comment';
  const model = 'model\' $(printf injected)';
  for (const cli of ['codex', 'claude', 'gemini'] as const) {
    const command = `${cli}() { printf '%s\\0' "$@"; }; ${taskAgentCommand(cli, prompt, model)}`;
    const actual = execFileSync('/bin/sh', ['-c', command], { encoding: 'utf8' }).split('\0').slice(0, -1);
    assert.deepEqual(actual, ['--model', model, ...(cli === 'gemini' ? ['--prompt-interactive'] : []), prompt]);
  }
});
