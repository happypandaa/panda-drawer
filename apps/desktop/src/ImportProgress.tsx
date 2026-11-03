import { t } from './i18n';
import { CheckCircle2 } from 'lucide-react';

export interface ImportProgressProps {
  steps: string[];
  currentStep: number;
  state?: 'running' | 'paused' | 'failed' | 'succeeded' | 'warning';
  label?: string;
  completedSteps?: number[];
}

// All import entry points describe real work with the same stage indicator.
// Explicit completedSteps lets cleanup run after a failure without pretending
// that the skipped installation stages succeeded.
export function ImportProgress({ steps, currentStep, state = 'running', label = t("导入进度"), completedSteps }: ImportProgressProps) {
  return <ol className="import-progress" aria-label={t(label)}>{steps.map((step, index) => {
    const complete = state === 'succeeded' || (completedSteps ? completedSteps.includes(index) : index < currentStep);
    const current = index === currentStep && state !== 'succeeded';
    return <li key={index} aria-current={current ? 'step' : undefined} className={complete ? 'is-complete' : current && (state === 'failed' || state === 'warning') ? 'needs-attention' : ''}>
      <span>{complete ? <CheckCircle2 size={16} aria-hidden="true" /> : index + 1}</span>{t(step)}
    </li>;
  })}</ol>;
}
