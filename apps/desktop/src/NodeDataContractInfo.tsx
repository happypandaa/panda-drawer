import type { NodeDataContract } from './node-app-types';

export function isNodeDataContract(value: unknown): value is NodeDataContract {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const declaration = value as Record<string, unknown>;
  return typeof declaration.id === 'string' && /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(declaration.id)
    && typeof declaration.version === 'number' && Number.isInteger(declaration.version)
    && declaration.version >= 1 && declaration.version <= 2147483647;
}

export function NodeDataContractInfo({ value, unavailable = false, legacyNotice, matched = false }: {
  value: NodeDataContract | null | undefined;
  unavailable?: boolean;
  legacyNotice?: string;
  matched?: boolean;
}) {
  if (unavailable || value === undefined || value !== null && !isNodeDataContract(value)) {
    return <div aria-label="数据声明"><p>数据声明：暂无法核对。</p></div>;
  }
  if (value === null) return <div aria-label="数据声明"><p>数据声明：未声明。{legacyNotice}</p></div>;
  return <div aria-label="数据声明">
    <p>数据声明：<code>{value.id}</code> · 版本 {value.version}{matched && ' · 数据声明匹配'}</p>
    <p>这是作者声明，平台未验证实际数据格式；普通更新只核对声明是否一致，不转换现有数据。</p>
  </div>;
}
