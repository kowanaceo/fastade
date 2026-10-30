import type { CliKind } from './session';

export interface AgentLaunchResult { action: 'existing' | 'resumeDirect' | 'direct'; cli: CliKind; }
