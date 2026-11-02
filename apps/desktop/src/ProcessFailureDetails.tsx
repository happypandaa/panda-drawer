import { t } from './i18n';
import { useEffect, useRef, useState } from 'react';
import { checkedProcessDiagnostics, processDiagnosticsText, processExitDescription } from './process-diagnostics';

export function ProcessFailureDetails({ value }: { value: unknown }) {
  const diagnostic = checkedProcessDiagnostics(value);
  const signature = JSON.stringify(diagnostic);
  const current = useRef(signature); current.current = signature;
  const alive = useRef(true);
  const [copying, setCopying] = useState(false);
  const [notice, setNotice] = useState('');
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { setCopying(false); setNotice(''); }, [signature]);
  if (!diagnostic) return null;
  const text = processDiagnosticsText(diagnostic);
  async function copy() {
    if (copying) return;
    const origin = signature;
    setCopying(true); setNotice('');
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(text);
      if (alive.current && current.current === origin) setNotice(t("错误详情已复制。"));
    } catch {
      if (alive.current && current.current === origin) setNotice(t("无法自动复制，请选择下方错误文字手动复制。"));
    } finally { if (alive.current && current.current === origin) setCopying(false); }
  }
  return <section className="process-failure-details" aria-label={t("具体错误")}>
    <div className="process-failure-heading"><strong>{t("具体错误")}</strong><button type="button" className="text-button" disabled={copying} onClick={() => void copy()}>{t("复制错误详情")}</button></div>
    <p><code>{diagnostic.command}</code> · {processExitDescription(diagnostic)}</p>
    <p>Node {diagnostic.nodeVersion} · npm {diagnostic.npmVersion}</p>
    {diagnostic.stderr && <><strong className="process-output-label">{t("错误输出")}</strong><pre tabIndex={0}>{diagnostic.stderr}</pre></>}
    {diagnostic.stdout && <><strong className="process-output-label">{t("标准输出")}</strong><pre tabIndex={0}>{diagnostic.stdout}</pre></>}
    {!diagnostic.stderr && !diagnostic.stdout && <p>{t("进程没有返回文字错误。")}</p>}
    {diagnostic.truncated && <p>{t("输出较长，部分内容已省略。")}</p>}
    {notice && <p role="status">{t(notice)}</p>}
  </section>;
}
