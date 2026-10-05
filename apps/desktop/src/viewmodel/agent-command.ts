import type { CliKind } from '../domain/session';

export interface AgentLaunch {
  cli: CliKind;
  requestedModel?: string;
}

/** Parses the small command subset fastade needs without evaluating shell
 * syntax. Quoted model values and both `--model=x` / `--model x` are kept. */
export function detectAgentLaunch(command: string): AgentLaunch | null {
  const launched = command.match(/^(?:command\s+)?(codex|claude|gemini)(?:\s|$)/);
  if (!launched) return null;
  const cli = launched[1] as CliKind;
  const args = command.slice(launched[0].length);
  const model = args.match(/(?:^|\s)(?:--model(?:=|\s+)|-m\s+)("(?:[^"\\]|\\.)*"|'(?:[^']|'"'"')*'|[^\s]+)/);
  return { cli, requestedModel: model ? unquote(model[1]) : undefined };
}

function unquote(value: string): string {
  if (value.startsWith('"') && value.endsWith('"')) {
    return value.slice(1, -1).replace(/\\(["\\])/g, '$1');
  }
  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1).replace(/'"'"'/g, "'");
  }
  return value;
}
