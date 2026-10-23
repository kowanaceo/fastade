import type { VoiceRecording } from '../application/voice-input';
import { MAX_VOICE_BYTES } from '../domain/voice-input';

export async function recordVoice(signal: AbortSignal): Promise<VoiceRecording> {
  if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') throw new Error('unsupported');
  const mimeType = ['audio/mp4', 'audio/webm;codecs=opus', 'audio/webm'].find((type) => MediaRecorder.isTypeSupported(type));
  if (!mimeType) throw new Error('unsupported');
  let stream: MediaStream;
  try { stream = await navigator.mediaDevices.getUserMedia({ audio: true }); }
  catch (error) {
    throw new Error(error instanceof DOMException && ['NotAllowedError', 'SecurityError'].includes(error.name) ? 'permission_denied' : 'microphone');
  }
  const release = (): void => stream.getTracks().forEach((track) => track.stop());
  if (signal.aborted) { release(); throw new Error('cancelled'); }
  let recorder: MediaRecorder;
  try { recorder = new MediaRecorder(stream, { mimeType }); }
  catch { release(); throw new Error('unsupported'); }
  let chunks: Blob[] = [];
  let bytes = 0;
  let failure = '';
  let cancelled = false;
  let stopping = false;
  let result: Blob | undefined;
  let resolveStop: ((value: Blob) => void) | undefined;
  let rejectStop: ((error: Error) => void) | undefined;
  const cleanup = (): void => { signal.removeEventListener('abort', cancel); release(); };
  function cancel(): void {
    cancelled = true;
    chunks = [];
    result = undefined;
    if (recorder.state !== 'inactive') recorder.stop();
    cleanup();
    rejectStop?.(new Error('cancelled'));
  }
  recorder.ondataavailable = ({ data }): void => {
    if (cancelled) return;
    bytes += data.size;
    if (bytes > MAX_VOICE_BYTES) {
      failure = 'too_large';
      chunks = [];
      if (recorder.state !== 'inactive') recorder.stop();
      cleanup();
    } else if (!failure) chunks.push(data);
  };
  recorder.onerror = (): void => {
    failure = 'microphone';
    if (recorder.state !== 'inactive') recorder.stop();
    cleanup();
    rejectStop?.(new Error(failure));
  };
  recorder.onstop = (): void => {
    cleanup();
    if (cancelled || failure) { rejectStop?.(new Error(failure || 'cancelled')); return; }
    result = new Blob(chunks, { type: recorder.mimeType });
    chunks = [];
    resolveStop?.(result);
  };
  signal.addEventListener('abort', cancel, { once: true });
  try { recorder.start(1000); }
  catch { cleanup(); throw new Error('microphone'); }
  return {
    cancel,
    stop: () => new Promise<Blob>((resolve, reject) => {
      if (cancelled || failure) { reject(new Error(failure || 'cancelled')); return; }
      if (result) { resolve(result); return; }
      if (stopping) { reject(new Error('microphone')); return; }
      stopping = true;
      resolveStop = resolve;
      rejectStop = reject;
      if (recorder.state !== 'inactive') recorder.stop();
    }),
  };
}
