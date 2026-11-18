import { gitRepositoryAddress } from './git-repository';
import type { ProjectInfo } from './types';

type SourceProject = Pick<ProjectInfo, 'ownership' | 'documentEpoch' | 'gitSource' | 'projectRoot'>;

interface ProjectSource {
  ownership: ProjectInfo['ownership'] | null;
  isLocal: boolean;
  kind: 'downloaded' | 'created' | 'local' | 'unknown';
  host?: 'GitHub' | 'Gitee';
  repository?: string;
  url?: string;
  path?: string;
}

export function projectSource(project: SourceProject): ProjectSource {
  const { ownership, documentEpoch, gitSource, projectRoot } = project;
  // An unreadable registration has a placeholder user owner and no document epoch.
  if (!documentEpoch || (ownership !== 'user' && ownership !== 'platform')) {
    return { ownership: null, isLocal: false, kind: 'unknown' };
  }
  if (ownership === 'user') {
    return { ownership, isLocal: true, kind: gitSource ? 'downloaded' : 'local', path: projectRoot };
  }
  if (!gitSource) return { ownership, isLocal: false, kind: 'created' };

  const url = typeof gitSource.url === 'string' ? gitSource.url : undefined;
  const address = url ? gitRepositoryAddress(url) : null;
  if (!address) return { ownership, isLocal: false, kind: 'downloaded', url };
  return {
    ownership,
    isLocal: false,
    kind: 'downloaded',
    host: address.provider === 'GitHub' ? 'GitHub' : 'Gitee',
    repository: new URL(address.url).pathname.slice(1).replace(/\.git$/, ''),
  };
}
