/**
 * Management UI visibility only. Native commands and the local service still
 * authorize every operation; these flags never imply a ready workspace,
 * successful bootstrap, healthy project, or completed task.
 */
export type ManagerPlatform = 'macos' | 'windows';

export interface ManagerCapabilities {
  readonly projects: boolean;
  readonly create: boolean;
  readonly openDesktop: boolean;
  readonly openBrowser: boolean;
  /** Display fields already returned by the platform's project status/list. */
  readonly readOnlyDetails: boolean;

  readonly metadata: boolean;
  readonly build: boolean;
  readonly handoff: boolean;
  readonly storage: boolean;
  readonly maintenance: boolean;
  readonly environmentDoctor: boolean;
  readonly environmentConfigure: boolean;
  readonly importProject: boolean;
  readonly exportProject: boolean;
  readonly copyProject: boolean;
  readonly deleteProject: boolean;
  readonly trash: boolean;
  readonly workspaceMigration: boolean;
  readonly historyCleanup: boolean;
  /** The full panel requires bootstrap/platform and storage information. */
  readonly aboutPlatform: boolean;
}

export const MAC_MANAGER_CAPABILITIES = Object.freeze({
  projects: true,
  create: true,
  openDesktop: true,
  openBrowser: true,
  readOnlyDetails: true,
  metadata: true,
  build: true,
  handoff: true,
  storage: true,
  maintenance: true,
  environmentDoctor: true,
  environmentConfigure: true,
  importProject: true,
  exportProject: true,
  copyProject: true,
  deleteProject: true,
  trash: true,
  workspaceMigration: true,
  historyCleanup: true,
  aboutPlatform: true,
} as const satisfies ManagerCapabilities);

export const WINDOWS_MANAGER_CAPABILITIES = Object.freeze({
  projects: true,
  create: true,
  openDesktop: true,
  openBrowser: true,
  readOnlyDetails: true,
  metadata: false,
  build: false,
  handoff: false,
  storage: false,
  maintenance: false,
  environmentDoctor: false,
  environmentConfigure: false,
  importProject: false,
  exportProject: false,
  copyProject: false,
  deleteProject: false,
  trash: false,
  workspaceMigration: false,
  historyCleanup: false,
  aboutPlatform: false,
} as const satisfies ManagerCapabilities);
