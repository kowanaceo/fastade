export type VoiceLanguage = '' | 'ko' | 'en';
export const MAX_VOICE_BYTES = 10 * 1024 * 1024;
export const MAX_VOICE_SECONDS = 60;

/** A reviewed dictation is a single submitted input, never terminal key codes. */
export function voiceTerminalInput(text: string): string {
  const input = text.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]+/g, ' ').trim();
  if (!input) throw new Error('empty');
  return `${input}\r`;
}
