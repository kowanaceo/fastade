import type { SessionRecord } from '../domain/session-record';
import { taskExecutionPrompt } from '../domain/task-execution.ts';

export interface TaskExecutionStore {
  claimTodoTasks(profileId: string, taskIds?: string[]): Promise<SessionRecord[]>;
  restoreClaimedTasks(tasks: SessionRecord[]): Promise<void>;
}

/** A single batch handoff; completion is reported by the executing agent. */
export async function executeSessionTasks(
  store: TaskExecutionStore,
  profileId: string,
  taskIds: string[] | undefined,
  send: (prompt: string) => Promise<void>,
): Promise<void> {
  const tasks = await store.claimTodoTasks(profileId, taskIds);
  if (!tasks.length) throw new Error('There are no to-do tasks to run. Refresh the task list.');
  try {
    await send(taskExecutionPrompt(profileId, tasks));
  } catch (cause) {
    try { await store.restoreClaimedTasks(tasks); }
    catch (rollback) {
      throw new Error(`${String(cause)} Task status could not be restored: ${String(rollback)}`);
    }
    throw cause;
  }
}
