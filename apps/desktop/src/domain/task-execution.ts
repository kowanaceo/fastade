import type { CliKind } from './session';
import type { SessionRecord } from './session-record';

/** One input line keeps task formatting inside JSON rather than sending PTY keys. */
export function taskExecutionPrompt(profileId: string, tasks: SessionRecord[]): string {
  if (!tasks.length || tasks.some((task) => task.kind !== 'task' || task.profileId !== profileId)) {
    throw new Error('Choose tasks from this session.');
  }
  const payload = JSON.stringify({ profileId, tasks: tasks.map(({ id, title, content }) => ({ id, title, content })) })
    .replace(/[\u007f-\u009f\u2028\u2029]/g, (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, '0')}`);
  return [
    'Execute the following saved tasks now, in the listed order, in this session’s project.',
    'Implement and verify each task. These tasks have been marked "in progress" because their execution was requested.',
    'When fastade record tools are available, use update_record with each exact task ID to mark it "done" only after completing and verifying it.',
    'If a task is blocked or incomplete, keep it "in progress", explain what remains in its record without removing its original requirements, and continue with the other listed tasks.',
    'Do not execute tasks outside this list. Report the result of each task.',
    payload,
  ].join(' ');
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'"'"'`)}'`;
}

/** Pass the initial request as one quoted argument, never as shell syntax. */
export function taskAgentCommand(cli: CliKind, prompt: string, model?: string): string {
  const modelArg = model ? ` --model ${shellQuote(model)}` : '';
  const promptArg = cli === 'gemini' ? ' --prompt-interactive' : '';
  return `${cli}${modelArg}${promptArg} ${shellQuote(prompt)}`;
}
