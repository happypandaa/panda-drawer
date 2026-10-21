import { invoke } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import { messageFrom } from './ui-utils';

interface GitRemoteCheckResult {
  url: string;
  provider: 'github' | 'gitee';
  transport: 'public-https';
  state: 'readable' | 'readable-no-head';
  head: { oid: string; symbolicRef: string | null } | null;
  git: { path: string; version: string };
  checkedAt: string;
}

function supportedAddress(value: string): boolean {
  return /^https:\/\/(?:github\.com|gitee\.com)\/[A-Za-z0-9][A-Za-z0-9_.-]{0,254}\/[A-Za-z0-9][A-Za-z0-9_.-]{0,254}\/?$/.test(value);
}

export function GitRemoteCheckPanel({ enabled, toolRevision, onPendingChange }: { enabled: boolean; toolRevision: number; onPendingChange: (pending: boolean) => void }) {
  const id = useId();
  const [url, setUrl] = useState('');
  const [result, setResult] = useState<GitRemoteCheckResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checkingUrl, setCheckingUrl] = useState<string | null>(null);
  const pending = useRef(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const currentToolRevision = useRef(toolRevision);
  currentToolRevision.current = toolRevision;
  const resultRevision = useRef(toolRevision);
  const address = url.trim();
  const valid = supportedAddress(address);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; generation.current += 1; };
  }, []);

  useEffect(() => {
    generation.current += 1;
    setResult(null);
    setError(null);
  }, [toolRevision]);

  function changeAddress(value: string) {
    generation.current += 1;
    setUrl(value);
    setResult(null);
    setError(null);
  }

  async function check() {
    if (!enabled || pending.current || !valid) return;
    pending.current = true;
    const request = ++generation.current;
    const revision = toolRevision;
    setCheckingUrl(address);
    setResult(null);
    setError(null);
    onPendingChange(true);
    try {
      const value = await invoke<GitRemoteCheckResult>('environment_git_remote_check', { input: { url: address } });
      if (!mounted.current || request !== generation.current || revision !== currentToolRevision.current) return;
      resultRevision.current = revision;
      setResult(value);
    } catch (cause) {
      if (mounted.current && request === generation.current && revision === currentToolRevision.current) setError(messageFrom(cause));
    } finally {
      pending.current = false;
      onPendingChange(false);
      if (mounted.current) setCheckingUrl(null);
    }
  }

  const visibleResult = resultRevision.current === toolRevision ? result : null;
  return <section className="git-remote-check" aria-labelledby={`${id}-title`}>
    <h3 id={`${id}-title`}>检查公开仓库访问</h3>
    <p>仅检查 GitHub 或 Gitee 的公开 HTTPS 仓库，不使用账号凭据。只读取远端 HEAD，不下载仓库、不推送，也不修改本地项目。</p>
    <form className="node-path-form" onSubmit={event => { event.preventDefault(); void check(); }}>
      <label htmlFor={`${id}-url`}>公开仓库地址</label>
      <div className="node-path-controls"><input id={`${id}-url`} value={url} onChange={event => changeAddress(event.target.value)} placeholder="https://github.com/owner/repository.git" autoComplete="off" autoCapitalize="none" spellCheck={false} aria-describedby={`${id}-hint`} /><button className="primary-button" type="submit" disabled={!enabled || Boolean(checkingUrl) || !valid}>{checkingUrl ? '检查中…' : error ? '重新检查' : '检查访问'}</button></div>
      <p id={`${id}-hint`}>{address && !valid ? '请填写标准 GitHub 或 Gitee HTTPS 仓库地址；不支持凭据、查询参数或仓库内页面。' : '输入地址后点击检查才会访问远端，最长等待 20 秒。'}</p>
      {!enabled && !checkingUrl && <p>请先完成 Git 工具检测，并确认工具可用。</p>}
    </form>
    {checkingUrl && <p className="workspace-operation-status" role="status">正在检查 <code>{checkingUrl}</code>，最长等待 20 秒；完成前请保留设置窗口。修改输入不会取消本次检查，原地址的结果将不再显示。</p>}
    {error && <p role="alert" className="environment-issue">{error}</p>}
    {visibleResult && <div className="operation-result" role="status">
      <p>{visibleResult.state === 'readable-no-head' ? '此公开仓库可读取，但本次未返回 HEAD。' : '此公开仓库可读取。'} 此结果只适用于本次地址，不代表账号认证或写入权限已验证。</p>
      <dl className="path-list">
        <div className="path-row"><dt>已检查地址</dt><dd><code>{visibleResult.url}</code></dd></div>
        <div className="path-row"><dt>检查时间</dt><dd><time dateTime={visibleResult.checkedAt}>{visibleResult.checkedAt}</time></dd></div>
        {visibleResult.head && <><div className="path-row"><dt>HEAD 提交</dt><dd><code>{visibleResult.head.oid}</code></dd></div>{visibleResult.head.symbolicRef && <div className="path-row"><dt>HEAD 引用</dt><dd><code>{visibleResult.head.symbolicRef}</code></dd></div>}</>}
        <div className="path-row"><dt>本次 Git {visibleResult.git.version}</dt><dd><code>{visibleResult.git.path}</code></dd></div>
      </dl>
    </div>}
  </section>;
}
