export interface ObjectLayout {
  kind: 'web' | 'local-node-source';
  source: {
    state: 'available' | 'missing' | 'unverified';
    path: string | null;
    reason: string;
  };
  runtime: {
    webRoot: string | null;
    backendRoot: string | null;
    external: boolean;
  };
  data: {
    root: string | null;
    database: string | null;
    attachments: string | null;
  };
  configurationPath: string | null;
  controlPath: string | null;
  build: {
    state: 'platform-build' | 'unavailable';
    reason: string;
  };
  versioning: {
    state: 'not-integrated' | 'source-git';
    reason: string;
    markers: {
      path: string;
      kind: 'directory' | 'file' | 'unsafe';
      scope: 'project' | 'source' | 'ancestor';
    }[];
    inspectionIssues: string[];
    database: { state: 'not-supported' | 'not-inspected'; reason: string };
  };
}
