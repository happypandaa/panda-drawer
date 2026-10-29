export interface GitRepositorySource { url: string; branch?: string | null }

export function gitRepositoryAddress(value: string): { url: string; provider: string; repository: string } | null {
  const match = /^(https:\/\/)(github\.com|gitee\.com)\/([A-Za-z0-9][A-Za-z0-9._-]{0,254})\/([A-Za-z0-9][A-Za-z0-9._-]{0,254})\/?$/.exec(value.trim());
  if (!match) return null;
  const repository = match[4].replace(/\.git$/, '');
  if (!repository) return null;
  return { url: `${match[1]}${match[2]}/${match[3]}/${repository}.git`, provider: match[2] === 'github.com' ? 'GitHub' : 'Gitee', repository };
}

export function sameGitRepository(left: GitRepositorySource, right: GitRepositorySource): boolean {
  const a = gitRepositoryAddress(left.url);
  const b = gitRepositoryAddress(right.url);
  return Boolean(a && b && a.url === b.url && (left.branch?.trim() || null) === (right.branch?.trim() || null));
}
