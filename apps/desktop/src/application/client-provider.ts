import type { DesktopClient } from './desktop-client';
import { TauriDesktopClient } from '../infrastructure/tauri-desktop-client';

export function createDesktopClient(): DesktopClient {
  return new TauriDesktopClient();
}
