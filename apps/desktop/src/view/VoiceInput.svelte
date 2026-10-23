<script lang="ts">
  import { onMount } from 'svelte';
  import { createDesktopClient } from '../application/client-provider';
  import { VoiceInputController, type VoiceState } from '../application/voice-input';
  import { recordVoice } from '../infrastructure/browser-voice-recorder';
  import { MAX_VOICE_SECONDS, type VoiceLanguage } from '../domain/voice-input';
  import { voiceMessages } from './voice-input-messages';

  let { sessionName, available, onSend, onClose }: {
    sessionName: string;
    available: boolean;
    onSend: (text: string) => Promise<void>;
    onClose: () => void;
  } = $props();
  const messages = voiceMessages();
  const client = createDesktopClient();
  let voiceState = $state<VoiceState>({ phase: 'idle', text: '', error: '', retryable: false });
  const languageKey = 'voice-input-language';
  function storedLanguage(): VoiceLanguage {
    try {
      const value = localStorage.getItem(languageKey);
      return value === 'ko' || value === 'en' ? value : '';
    } catch { return ''; }
  }
  let language = $state<VoiceLanguage>(storedLanguage());
  let text = $state('');
  let seconds = $state(0);
  let dialog: HTMLDialogElement;
  const controller = new VoiceInputController({
    record: recordVoice,
    transcribe: (audio, speechLanguage, signal) => client.transcribeVoice(audio, speechLanguage, signal),
    send: (value) => onSend(value),
  }, (next) => {
    voiceState = next;
    if (next.phase === 'review') text = next.text;
  }, () => onClose());
  const busy = $derived(['permission', 'recording', 'transcribing', 'sending'].includes(voiceState.phase));
  const errorMessage = $derived(voiceState.error ? messages.errors[voiceState.error as keyof typeof messages.errors] ?? messages.errors.fallback : '');

  function close(): void {
    if (voiceState.phase === 'sending') return;
    controller.dispose();
    onClose();
  }

  $effect(() => {
    try { localStorage.setItem(languageKey, language); } catch { /* storage unavailable */ }
  });

  $effect(() => {
    if (voiceState.phase !== 'recording') return;
    seconds = 0;
    const started = Date.now();
    const timer = window.setInterval(() => {
      seconds = Math.floor((Date.now() - started) / 1000);
      if (seconds >= MAX_VOICE_SECONDS) void controller.stop();
    }, 250);
    return () => window.clearInterval(timer);
  });

  onMount(() => {
    dialog.showModal();
    void controller.start(language);
    return () => { controller.dispose(); dialog.close(); };
  });
</script>

<dialog bind:this={dialog} aria-labelledby="voice-title" oncancel={(event) => { event.preventDefault(); close(); }}>
  <header><div><h2 id="voice-title">{messages.title}</h2><small>{sessionName}</small></div><button type="button" onclick={close} disabled={voiceState.phase === 'sending'} aria-label={messages.close}>×</button></header>
  <label for="voice-language">{messages.language}</label>
  <select id="voice-language" bind:value={language} disabled={busy}>
    <option value="">{messages.auto}</option><option value="ko">{messages.korean}</option><option value="en">{messages.english}</option>
  </select>
  <div class="voice-status" role="status" aria-live="polite">
    {#if voiceState.phase === 'permission'}{messages.permission}
    {:else if voiceState.phase === 'recording'}<span class="recording-dot"></span>{messages.recording} · {Math.min(seconds, MAX_VOICE_SECONDS)} / {MAX_VOICE_SECONDS}s
    {:else if voiceState.phase === 'transcribing'}{messages.transcribing}
    {:else if voiceState.phase === 'sending'}{messages.sending}
    {:else if voiceState.phase === 'idle'}{messages.idle}{/if}
  </div>
  {#if voiceState.phase === 'review' || voiceState.phase === 'sending'}
    <label for="voice-text">{messages.review}</label>
    <textarea id="voice-text" bind:value={text} disabled={voiceState.phase === 'sending'} rows="6"></textarea>
    <p class="hint">{messages.hint}</p>
  {/if}
  {#if errorMessage}<p class="voice-error" role="alert">{errorMessage}</p>{/if}
  {#if !available}<p class="voice-error" role="alert">{messages.unavailableSession}</p>{/if}
  <footer>
    <button type="button" onclick={close} disabled={voiceState.phase === 'sending'}>{messages.close}</button>
    {#if voiceState.phase === 'recording'}
      <button class="primary" type="button" onclick={() => void controller.stop()}>{messages.stop}</button>
    {:else if !busy}
      <button type="button" onclick={() => void controller.start(language)}>{voiceState.phase === 'idle' ? messages.start : messages.again}</button>
      {#if voiceState.phase === 'error' && voiceState.retryable}<button type="button" onclick={() => void controller.retry(language)}>{messages.retry}</button>{/if}
      {#if voiceState.phase === 'review'}<button class="primary" type="button" disabled={!text.trim() || !available} onclick={() => { void controller.send(text).then((sent) => { if (sent) onClose(); }); }}>{messages.send}</button>{/if}
    {/if}
  </footer>
</dialog>

<style>
  dialog { width: min(500px, calc(100vw - 48px)); max-height: calc(100vh - 48px); overflow: auto; box-sizing: border-box; padding: 20px; border: 1px solid #52755b; border-radius: 10px; background: #101812; color: #d6e0d9; box-shadow: 0 16px 60px #000a; }
  dialog::backdrop { background: #0009; }
  header, footer { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  header { margin-bottom: 20px; }
  header div { min-width: 0; }
  h2 { margin: 0 0 5px; font-size: 17px; }
  small { display: block; overflow-wrap: anywhere; color: #a9bcaf; }
  label { display: block; margin: 12px 0 7px; font-size: 12px; }
  select, textarea { box-sizing: border-box; width: 100%; padding: 9px; border: 1px solid #425648; border-radius: 5px; background: #0b100d; color: inherit; font: inherit; }
  textarea { resize: vertical; min-height: 100px; }
  .voice-status, .hint, .voice-error { font-size: 12px; line-height: 1.6; overflow-wrap: anywhere; }
  .voice-status { margin-top: 16px; }
  .hint { color: #a9bcaf; }
  .voice-error { color: #ffb4aa; }
  .recording-dot { display: inline-block; width: 8px; height: 8px; margin-right: 6px; border-radius: 50%; background: #ff8175; }
  footer { justify-content: flex-end; flex-wrap: wrap; margin-top: 20px; }
  button { padding: 7px 10px; border: 1px solid #425648; border-radius: 5px; background: #1c2b21; color: inherit; }
  button.primary { border-color: #8eefaa; color: #8eefaa; }
  button:disabled { opacity: .45; }
</style>
