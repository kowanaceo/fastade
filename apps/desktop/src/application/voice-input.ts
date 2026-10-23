import { MAX_VOICE_BYTES, type VoiceLanguage } from '../domain/voice-input.ts';

export interface VoiceRecording {
  stop(): Promise<Blob>;
  cancel(): void;
}

export type VoicePhase = 'idle' | 'permission' | 'recording' | 'transcribing' | 'review' | 'sending' | 'error';
export interface VoiceState { phase: VoicePhase; text: string; error: string; retryable: boolean }
export interface VoiceDependencies {
  record(signal: AbortSignal): Promise<VoiceRecording>;
  transcribe(audio: Blob, language: VoiceLanguage, signal: AbortSignal): Promise<string>;
  send(text: string): Promise<void>;
}

/** Owns one recording and its pinned destination. Late results cannot revive a closed dialog. */
export class VoiceInputController {
  state: VoiceState = { phase: 'idle', text: '', error: '', retryable: false };
  private active = true;
  private version = 0;
  private abort = new AbortController();
  private recording?: VoiceRecording;
  private audio?: Blob;
  private language: VoiceLanguage = '';
  private dependencies: VoiceDependencies;
  private changed: (state: VoiceState) => void;
  private sent: () => void;

  constructor(dependencies: VoiceDependencies, changed: (state: VoiceState) => void, sent: () => void = () => {}) {
    this.dependencies = dependencies;
    this.changed = changed;
    this.sent = sent;
  }

  private update(patch: Partial<VoiceState>): void {
    if (!this.active) return;
    this.state = { ...this.state, ...patch };
    this.changed(this.state);
  }

  async start(language: VoiceLanguage): Promise<void> {
    if (!this.active || ['permission', 'recording', 'transcribing', 'sending'].includes(this.state.phase)) return;
    const version = ++this.version;
    this.abort.abort();
    this.abort = new AbortController();
    this.recording?.cancel();
    this.audio = undefined;
    this.language = language;
    this.update({ phase: 'permission', error: '', retryable: false, text: '' });
    try {
      const recording = await this.dependencies.record(this.abort.signal);
      if (!this.active || version !== this.version) { recording.cancel(); return; }
      this.recording = recording;
      this.update({ phase: 'recording' });
    } catch (error) {
      if (version === this.version) this.fail(error);
    }
  }

  async stop(): Promise<void> {
    if (!this.active || this.state.phase !== 'recording' || !this.recording) return;
    this.update({ phase: 'transcribing' });
    try {
      const audio = await this.recording.stop();
      this.recording = undefined;
      if (!this.active) return;
      this.audio = audio;
      if (!this.audio.size) throw new Error('empty_audio');
      if (this.audio.size > MAX_VOICE_BYTES) throw new Error('too_large');
      await this.transcribe();
    } catch (error) { this.fail(error); }
  }

  async retry(language: VoiceLanguage = this.language): Promise<void> {
    if (this.active && this.state.phase === 'error' && this.state.retryable) {
      this.language = language;
      await this.transcribe();
    }
  }

  private async transcribe(): Promise<void> {
    if (!this.audio) return;
    this.update({ phase: 'transcribing', error: '', retryable: false });
    try {
      const text = await this.dependencies.transcribe(this.audio, this.language, this.abort.signal);
      if (!text.trim()) throw new Error('empty_text');
      this.update({ phase: 'review', text });
      // Dictation is sent immediately; the review state remains only as a fallback if sending fails.
      if (await this.send(text)) this.sent();
    } catch (error) { this.fail(error); }
  }

  async send(text: string): Promise<boolean> {
    if (!this.active || this.state.phase !== 'review' || !text.trim()) return false;
    this.update({ phase: 'sending', text, error: '' });
    try {
      await this.dependencies.send(text);
      this.dispose();
      return true;
    } catch (error) {
      this.update({ phase: 'review', error: error instanceof Error ? error.message : String(error) });
      return false;
    }
  }

  private fail(error: unknown): void {
    this.recording?.cancel();
    this.recording = undefined;
    this.update({ phase: 'error', error: error instanceof Error ? error.message : String(error), retryable: Boolean(this.audio?.size && this.audio.size <= MAX_VOICE_BYTES) });
  }

  dispose(): void {
    this.active = false;
    ++this.version;
    this.abort.abort();
    this.recording?.cancel();
    this.recording = undefined;
    this.audio = undefined;
  }
}
