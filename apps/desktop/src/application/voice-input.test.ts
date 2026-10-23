import assert from 'node:assert/strict';
import test from 'node:test';
import { VoiceInputController, type VoiceDependencies, type VoiceRecording } from './voice-input.ts';
import { MAX_VOICE_BYTES, voiceTerminalInput } from '../domain/voice-input.ts';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture(overrides: Partial<VoiceDependencies> = {}) {
  const sent: string[] = [];
  let cancelled = 0;
  const recording: VoiceRecording = {
    stop: async () => new Blob(['audio'], { type: 'audio/mp4' }),
    cancel: () => { cancelled++; },
  };
  const controller = new VoiceInputController({
    record: async () => recording,
    transcribe: async () => '한국어 and English',
    send: async (text) => { sent.push(text); },
    ...overrides,
  }, () => {});
  return { controller, sent, recording, cancelled: () => cancelled };
}

test('recognized text is sent immediately without a review step', async () => {
  const { controller, sent } = fixture();
  await controller.start('ko');
  assert.equal(controller.state.phase, 'recording');
  await controller.stop();
  assert.deepEqual(sent, ['한국어 and English']);
});

test('cancelling a pending permission request releases a microphone granted later', async () => {
  const pending = deferred<VoiceRecording>();
  const f = fixture({ record: () => pending.promise });
  const start = f.controller.start('');
  f.controller.dispose();
  pending.resolve(f.recording);
  await start;
  assert.equal(f.cancelled(), 1);
  assert.deepEqual(f.sent, []);
});

test('cancel aborts transcription and ignores late results without sending', async () => {
  const pending = deferred<string>();
  let signal: AbortSignal | undefined;
  const f = fixture({ transcribe: (_audio, _language, abort) => { signal = abort; return pending.promise; } });
  await f.controller.start('en');
  const stop = f.controller.stop();
  await Promise.resolve();
  f.controller.dispose();
  assert.equal(signal?.aborted, true);
  pending.resolve('late transcript');
  await stop;
  assert.notEqual(f.controller.state.phase, 'review');
  assert.equal(await f.controller.send('late transcript'), false);
  assert.deepEqual(f.sent, []);
});

test('retries the same audio after a server or network failure, preserving the chosen speech language', async () => {
  const uploads: { audio: Blob; language: string }[] = [];
  const f = fixture({ transcribe: async (audio, language) => {
    uploads.push({ audio, language });
    if (uploads.length === 1) throw new Error('unavailable');
    return 'recovered';
  } });
  await f.controller.start('en');
  await f.controller.stop();
  assert.equal(f.controller.state.error, 'unavailable');
  assert.equal(f.controller.state.retryable, true);
  await f.controller.retry();
  assert.equal(uploads[0].audio, uploads[1].audio);
  assert.deepEqual(uploads.map((upload) => upload.language), ['en', 'en']);
  assert.equal(f.controller.state.text, 'recovered');
});

test('automatic send failure falls back to review with the text; retrying cannot double send', async () => {
  let calls = 0;
  const f = fixture({ send: async () => { calls++; if (calls === 1) throw new Error('session'); } });
  await f.controller.start('');
  await f.controller.stop();
  assert.equal(calls, 1);
  assert.equal(f.controller.state.phase, 'review');
  assert.equal(f.controller.state.text, '한국어 and English');
  assert.equal(f.controller.state.error, 'session');
  assert.equal(await f.controller.send('edited'), true);
  assert.equal(calls, 2);
});

test('permission denial permits a new recording without any upload', async () => {
  let attempts = 0;
  let uploads = 0;
  const f = fixture({
    record: async () => { attempts++; throw new Error('permission_denied'); },
    transcribe: async () => { uploads++; return ''; },
  });
  await f.controller.start('ko');
  assert.equal(f.controller.state.error, 'permission_denied');
  assert.equal(f.controller.state.retryable, false);
  await f.controller.start('en');
  assert.equal(attempts, 2);
  assert.equal(uploads, 0);
});

test('empty and oversize audio never reaches the transcription service', async () => {
  for (const [audio, error] of [[new Blob(), 'empty_audio'], [new Blob([new Uint8Array(MAX_VOICE_BYTES + 1)]), 'too_large']] as const) {
    let uploads = 0;
    const f = fixture({
      record: async () => ({ stop: async () => audio, cancel: () => {} }),
      transcribe: async () => { uploads++; return 'text'; },
    });
    await f.controller.start('');
    await f.controller.stop();
    assert.equal(f.controller.state.error, error);
    assert.equal(f.controller.state.retryable, false);
    assert.equal(uploads, 0);
  }
});

test('empty recognition cannot enter review or submit input', async () => {
  const f = fixture({ transcribe: async () => '  ' });
  await f.controller.start('');
  await f.controller.stop();
  assert.equal(f.controller.state.error, 'empty_text');
  assert.equal(await f.controller.send('  '), false);
  assert.deepEqual(f.sent, []);
});

test('reviewed Korean and English reach the terminal as one submission without control keys or extra lines', () => {
  assert.equal(voiceTerminalInput('  한국어\nEnglish\r\n다음 줄\t끝  '), '한국어 English 다음 줄 끝\r');
  const input = voiceTerminalInput('hello\x03\x1b\x7f\x9bworld');
  assert.equal(input, 'hello world\r');
  assert.equal(input.match(/\r/g)?.length, 1);
  assert.throws(() => voiceTerminalInput('\r\n\t\x1b'), /empty/);
});
