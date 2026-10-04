import assert from 'node:assert/strict';
import test from 'node:test';
import { validateRecordDraft } from './session-record.ts';

test('notes preserve content formatting and reject task status', () => {
  assert.deepEqual(validateRecordDraft({ kind: 'note', title: '  Design  ', content: '  code\n\n' }), {
    kind: 'note', title: 'Design', content: '  code\n\n',
  });
  assert.throws(() => validateRecordDraft({ kind: 'note', title: 'Design', content: '', status: 'done' }), /do not have a status/);
});

test('tasks require one of the three agreed statuses', () => {
  for (const status of ['todo', 'in progress', 'done']) {
    assert.equal(validateRecordDraft({ kind: 'task', title: 'Ship', content: '', status }).kind, 'task');
  }
  for (const status of [undefined, null, '', 'pending', 'in_progress', {}, 1]) {
    assert.throws(() => validateRecordDraft({ kind: 'task', title: 'Ship', content: '', status }), /valid task status/);
  }
});

test('malformed records and blank titles fail before persistence', () => {
  for (const value of [null, [], { kind: 'other' }, { kind: 'note', title: ' \n ', content: '' }, { kind: 'note', title: 'Title', content: 1 }]) {
    assert.throws(() => validateRecordDraft(value));
  }
});
