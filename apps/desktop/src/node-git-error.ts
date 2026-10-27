import { messageFrom } from './ui-utils';

export interface NodeGitFailureRequest {
  operation: string;
  operationId?: string;
}
function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
}
const restoreId = /^restore_[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;

// A committed business receipt does not prove process/resource closure. Reads,
// unknown results and receipts for another operation never become success text.
export function nodeGitFailureMessage(cause: unknown, request: NodeGitFailureRequest): string {
  const details = record(record(cause)?.details);
  const result = record(details?.result);
  let receipt = '';
  if (details?.operationOutcome === 'committed' && result?.operationOutcome === 'committed') {
    if ((request.operation === 'restore-apply' || request.operation === 'restore-recover')
      && result.sourceOnly === true && (result.state === 'completed' || result.state === 'undone')
      && typeof result.operationId === 'string' && restoreId.test(result.operationId)
      && (request.operation !== 'restore-recover' || result.operationId === request.operationId)) {
      receipt = '源码操作已提交';
    }
  }
  const message = messageFrom(cause);
  if (receipt) return `${receipt}，最后状态尚待核对，请勿重复提交。${details?.processResourceClosed === false ? '相关任务仍待收尾，请保留现场。' : ''} ${message}`;
  if (['restore-apply', 'restore-recover'].includes(request.operation) || details?.operationOutcome === 'unknown' || details?.processResourceClosed === false) return `${message} 操作结果仍待核对，请刷新状态并保留现场，勿重复提交。`;
  return message;
}
