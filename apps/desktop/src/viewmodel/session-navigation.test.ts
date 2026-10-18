import assert from 'node:assert/strict';
import test from 'node:test';
import { SessionSelectionHistory } from './session-navigation.ts';

test('returns the last used session rather than its position in the list', () => {
  const history = new SessionSelectionHistory();
  for (const id of ['a', 'c', 'b']) history.record(id);
  assert.equal(history.previous('b', ['a', 'b', 'c']), 'c');
});

test('repeated switches alternate between the two most recent sessions', () => {
  const history = new SessionSelectionHistory();
  for (const id of ['a', 'b', 'c', 'c']) history.record(id);
  for (const [current, expected] of [['c', 'b'], ['b', 'c'], ['c', 'b']]) {
    const previous = history.previous(current, ['a', 'b', 'c']);
    assert.equal(previous, expected);
    history.record(previous);
  }
});

test('skips sessions that have closed or stopped', () => {
  const history = new SessionSelectionHistory();
  for (const id of ['a', 'b', 'c', 'd']) history.record(id);
  assert.equal(history.previous('d', ['a', 'd']), 'a');
  history.record('a');
  assert.equal(history.previous('a', ['a']), null);
});

test('does nothing until another session has been selected', () => {
  const history = new SessionSelectionHistory();
  assert.equal(history.previous(null, ['a', 'b']), null);
  history.record(null);
  history.record('a');
  assert.equal(history.previous('a', ['a', 'b']), null);
});
