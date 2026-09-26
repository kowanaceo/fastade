import type { CliKind } from '../domain/session';

export type AgentActivity = 'working' | 'waiting' | 'idle';

const WORKING_PATTERN = /[⠀-⣿]|(?:esc|ctrl\+c) to interrupt|thinking(?:…|\.\.\.)|generating(?:…|\.\.\.)|running(?:…|\.\.\.)|working(?:…|\.\.\.|\s*\()|waiting(?:…|\.\.\.|\s*\(|\s+for\b)|combobulating|reticulating/i;
const WAITING_DIRECT_PATTERN = /\(y\/n\)|\[y\/n\]|press enter to (?:continue|confirm)|enter to select[^\n]*(?:navigate|esc to cancel)|needs your approval|approval needed in|permission (?:required|needed)|waiting for (?:approval|permission|your (?:approval|input|response))/i;
const WAITING_QUESTION_PATTERN = /(?:do you want to|would you like to|are you sure you want to|allow|approve|grant)\b[^\n]{0,160}(?:\?|permission|access|command|tool|action)/i;
const WAITING_CHOICE_PATTERN = /(?:^|\n)\s*(?:[>›❯]\s*|\d+[.)]\s*)(?:yes|no|allow|approve|continue)\b|yes,? and don['’]t ask again|esc to cancel|tab to amend/i;

// Codex prints this after a turn has finished. Claude Code uses the same
// stable suffix while varying the playful verb before it between releases.
const CODEX_IDLE_PATTERN = /\bworked for\s+\d/i;
const CLAUDE_IDLE_PATTERN = /\bworked for\s+\d|(?:^|\n)\s*[✻✽✶✢·●◆*]\s*[\p{L}][\p{L} -]{0,32}\s+for\s+\d+(?:\.\d+)?\s*(?:ms|s|sec(?:ond)?s?|m|min(?:ute)?s?|h|hours?)\b/imu;

function lastPatternIndex(value: string, pattern: RegExp): number {
  const matches = value.matchAll(new RegExp(pattern.source, `${pattern.flags.replace('g', '')}g`));
  let last = -1;
  for (const match of matches) last = match.index;
  return last;
}

/** Classifies only explicit UI cues on the terminal's current screen. No cue
 * means "keep the previous state"; output silence is never proof of idleness. */
export function detectAgentActivity(cli: CliKind, screen: string): AgentActivity | undefined {
  const normalized = screen.replace(/\u00a0/g, ' ');
  const directWaitingIndex = lastPatternIndex(normalized, WAITING_DIRECT_PATTERN);
  const questionIndex = lastPatternIndex(normalized, WAITING_QUESTION_PATTERN);
  const choiceIndex = lastPatternIndex(normalized, WAITING_CHOICE_PATTERN);
  const waitingIndex = Math.max(
    directWaitingIndex,
    questionIndex >= 0 && choiceIndex >= 0 ? Math.max(questionIndex, choiceIndex) : -1,
  );
  const workingIndex = lastPatternIndex(normalized, WORKING_PATTERN);
  const idlePattern = cli === 'claude' ? CLAUDE_IDLE_PATTERN : cli === 'codex' ? CODEX_IDLE_PATTERN : /$a/;
  const idleIndex = lastPatternIndex(normalized, idlePattern);

  const latest = Math.max(waitingIndex, workingIndex, idleIndex);
  if (latest < 0) return undefined;
  if (waitingIndex === latest) return 'waiting';
  if (workingIndex === latest) return 'working';
  return 'idle';
}
