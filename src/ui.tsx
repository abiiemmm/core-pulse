import { getLocale, t } from './i18n';
import { useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { AlertCircle, ArrowUpRight, Check, ChevronRight, Minus, X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { Metric } from './types';

export const formatNumber = (value: number | null | undefined, decimals = 0) => value == null ? '—' : value.toLocaleString(getLocale(), { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
export const formatBytes = (value: number) => value >= 1024 ** 3 ? `${formatNumber(value / 1024 ** 3, 1)} GB` : value >= 1024 ** 2 ? `${formatNumber(value / 1024 ** 2, 1)} MB` : `${formatNumber(value / 1024, 0)} KB`;
export const metricText = (metric?: Metric, decimals = 1) => metric?.value == null ? t("Tidak tersedia") : `${formatNumber(metric.value, decimals)}${metric.unit === '%' || metric.unit === '°C' ? '' : ' '}${metric.unit}`;
export const timeText = (time?: string) => time ? new Date(time).toLocaleTimeString(getLocale(), { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }) : '—';
export const dateText = (time: string) => new Date(time).toLocaleString(getLocale(), { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false });

export function PageHeader({ title, description, actions }: { title: string; description: string; actions?: ReactNode }) {
  return <header className="page-header"><div><h1>{t(title)}</h1><p>{t(description)}</p></div>{actions && <div className="page-actions">{actions}</div>}</header>;
}

export function SectionHeader({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return <div className="section-header"><div><h2>{t(title)}</h2>{description && <p>{t(description)}</p>}</div>{action}</div>;
}

const STATUS_LABELS: Record<string, string> = { supported: 'Tersedia', unsupported: 'Tidak didukung', requires_permission: 'Perlu izin', unknown: 'Belum terdeteksi', active: 'Aktif', pending: 'Menunggu', restoring: 'Memulihkan', conflict: 'Perlu ditinjau', completed: 'Selesai', cancelled: 'Dibatalkan', partial: 'Sebagian selesai', failed: 'Gagal' };
export function Badge({ status, children }: { status: string; children?: ReactNode }) {
  const good = ['supported', 'active', 'completed'].includes(status);
  return <span className={`badge ${good ? 'badge-solid' : ''}`}><span aria-hidden="true">{good ? <Check size={11} /> : ['conflict', 'failed', 'requires_permission'].includes(status) ? <AlertCircle size={11} /> : <Minus size={11} />}</span>{children ?? t(STATUS_LABELS[status] ?? status)}</span>;
}

export function DataRow({ label, value, detail, icon: Icon }: { label: string; value: ReactNode; detail?: string; icon?: LucideIcon }) {
  return <div className="data-row">{Icon && <Icon size={16} className="muted" />}<div className="data-label"><span>{t(label)}</span>{detail && <small>{t(detail)}</small>}</div><div className="data-value">{typeof value === 'string' ? t(value) : value}</div></div>;
}

export function EmptyState({ icon: Icon, title, description }: { icon: LucideIcon; title: string; description: string }) {
  return <div className="empty-state"><Icon size={25} strokeWidth={1.3} /><strong>{t(title)}</strong><p>{t(description)}</p></div>;
}

export function Segmented<T extends string | number>({ label, value, options, onChange, disabled = false }: { label: string; value: T; options: { value: T; label: string }[]; onChange: (value: T) => void; disabled?: boolean }) {
  return <div className="segmented" role="group" aria-label={t(label)}>{options.map(option => <button type="button" key={option.value} aria-pressed={value === option.value} className={value === option.value ? 'is-selected' : ''} disabled={disabled} onClick={() => onChange(option.value)}>{t(option.label)}</button>)}</div>;
}

export function Toggle({ label, checked, onChange, disabled = false }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  return <button type="button" className={`toggle ${checked ? 'is-on' : ''}`} role="switch" aria-checked={checked} aria-label={t(label)} disabled={disabled} onClick={() => onChange(!checked)}><span /></button>;
}

export function Meter({ value, label }: { value: number | null; label: string }) {
  return <div className={`meter ${value == null ? 'meter-empty' : ''}`} role={value == null ? undefined : 'meter'} aria-label={t(label)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={value ?? undefined}><span style={{ width: `${Math.min(100, Math.max(0, value ?? 0))}%` }} /></div>;
}

type Plot = { key: string; label: string; values: (number | null)[] };
function plotSegments(values: (number | null)[], width: number, height: number, maximum = 100) {
  const segments: { x: number; y: number }[][] = [];
  let current: { x: number; y: number }[] = [];
  values.forEach((value, index) => {
    if (value == null) {
      if (current.length) segments.push(current);
      current = [];
    } else {
      current.push({ x: values.length === 1 ? width : index / (values.length - 1) * width, y: height - Math.min(maximum, Math.max(0, value)) / maximum * height });
    }
  });
  if (current.length) segments.push(current);
  return segments;
}

export function Sparkline({ values, plot = 'cpu' }: { values: (number | null)[]; plot?: string }) {
  const segments = plotSegments(values, 200, 36);
  return <svg className={`sparkline plot-${plot}`} viewBox="0 0 200 40" preserveAspectRatio="none" aria-hidden="true">{segments.length ? segments.map((segment, index) => segment.length === 1 ? <circle key={index} cx={segment[0].x} cy={segment[0].y + 2} r={2} /> : <polyline key={index} points={segment.map(p => `${p.x},${p.y + 2}`).join(' ')} />) : <line className="empty-line" x1="0" x2="200" y1="30" y2="30" />}</svg>;
}

export function MetricTile({ title, metric, detail, icon: Icon, values, plot, onClick }: { title: string; metric?: Metric; detail: string; icon: LucideIcon; values: (number | null)[]; plot: string; onClick: () => void }) {
  const available = metric?.value != null;
  return <button className="metric-tile" onClick={onClick} aria-label={t("Lihat {0}", { 0: title })}><div className="metric-label"><span><Icon size={16} />{t(title)}</span><ArrowUpRight size={14} className="metric-arrow" /></div><div className={`metric-number ${!available ? 'number-unavailable' : ''}`}>{formatNumber(metric?.value)}{available && <span>{metric?.unit}</span>}</div><p title={t(detail)}>{t(detail)}</p><Sparkline values={values} plot={plot} /><div className="metric-caption">{!metric ? t("Menunggu sampel") : available ? t("Penggunaan saat ini") : t("Sensor tidak tersedia")}</div></button>;
}

export function HistoryChart({ plots, times, tall = false, maximum = 100, unit = '%', countLabel }: { plots: Plot[]; times: string[]; tall?: boolean; maximum?: number; unit?: string; countLabel?: string }) {
  const [hover, setHover] = useState<number | null>(null);
  const hasData = plots.some(plot => plot.values.some(value => value != null));
  const selected = hover == null ? null : Math.max(0, Math.min(times.length - 1, hover));
  return <div className={`history-chart ${tall ? 'chart-tall' : ''}`}>
    <div className="plot-scale">{[1, .75, .5, .25, 0].map(fraction => <span key={fraction}>{formatNumber(maximum * fraction, maximum < 5 ? 1 : 0)}{unit === '%' ? '%' : ''}</span>)}</div>
    <div className="plot-area" onPointerMove={event => { if (!times.length) return; const bounds = event.currentTarget.getBoundingClientRect(); setHover(Math.round((event.clientX - bounds.left) / bounds.width * (times.length - 1))); }} onPointerLeave={() => setHover(null)}>
      <svg viewBox="0 0 1000 200" preserveAspectRatio="none" role="img" aria-label={t("Grafik penggunaan {0}. {1} sampel.", { 0: plots.map(plot => plot.label).join(', '), 1: times.length })}>
        {[0, 50, 100, 150, 200].map(y => <line className="plot-grid" key={y} x1="0" x2="1000" y1={y} y2={y} />)}
        {plots.map((plot, plotIndex) => <g className={`plot-${plot.key}`} key={plot.key}>{plotSegments(plot.values, 1000, 200, maximum).map((segment, index) => <g key={index}>{plotIndex === 0 && segment.length > 1 && <polygon className="plot-fill" points={`${segment[0].x},200 ${segment.map(p => `${p.x},${p.y}`).join(' ')} ${segment.at(-1)!.x},200`} />}{segment.length === 1 ? <circle className="plot-point" cx={segment[0].x} cy={segment[0].y} r={3} /> : <polyline className="plot-line" points={segment.map(p => `${p.x},${p.y}`).join(' ')} />}</g>)}</g>)}
      </svg>
      {!hasData && <div className="chart-empty"><span>{t("Belum ada pengukuran yang tersedia")}</span><small>{t("Grafik akan muncul saat sensor mengirim data.")}</small></div>}
      {selected != null && hasData && <><div className="plot-cursor" style={{ left: `${times.length === 1 ? 100 : selected / (times.length - 1) * 100}%` }} /><div className="plot-tooltip" style={{ left: `${Math.min(78, Math.max(2, times.length === 1 ? 78 : selected / (times.length - 1) * 100))}%` }}><strong>{dateText(times[selected])}</strong>{plots.map(plot => <span key={plot.key}>{t(plot.label)}<b>{formatNumber(plot.values[selected], 1)}{plot.values[selected] != null ? ` ${unit}` : ''}</b></span>)}</div></>}
    </div>
    <div className="plot-time"><span>{times[0] && (Date.parse(times.at(-1)!) - Date.parse(times[0]) > 3600000 ? dateText(times[0]) : timeText(times[0]))}</span><span>{countLabel ?? (times.length ? t("{0} sampel", { 0: times.length }) : t("Menunggu data"))}</span><span>{times.at(-1) && (Date.parse(times.at(-1)!) - Date.parse(times[0]) > 3600000 ? dateText(times.at(-1)!) : timeText(times.at(-1)))}</span></div>
  </div>;
}

export function ChartLegend({ items }: { items: { key: string; label: string }[] }) {
  return <div className="chart-legend">{items.map(item => <span key={item.key}><i className={`legend-${item.key}`} />{t(item.label)}</span>)}</div>;
}

export function Dialog({ title, description, children, footer, onClose, className = '' }: { title: string; description?: string; children?: ReactNode; footer?: ReactNode; onClose: () => void; className?: string }) {
  const id = useId();
  const ref = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const element = ref.current;
    const selectable = () => Array.from(element?.querySelectorAll<HTMLElement>('button:not(:disabled), input, select, [tabindex="0"]') ?? []).filter(node => node.offsetParent !== null);
    const target = element?.querySelector<HTMLElement>('[data-autofocus]') ?? selectable()[0];
    target?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); closeRef.current(); }
      if (event.key === 'Tab') {
        const choices = selectable();
        if (!choices.length) { event.preventDefault(); return; }
        const first = choices[0], last = choices.at(-1)!;
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    };
    element?.addEventListener('keydown', keydown);
    return () => { element?.removeEventListener('keydown', keydown); previous?.focus(); };
  }, []);
  return <div className="dialog-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}><div ref={ref} className={`dialog ${className}`} role="dialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={description ? `${id}-description` : undefined}><div className="dialog-heading"><h2 id={`${id}-title`}>{t(title)}</h2><button className="icon-button" onClick={onClose} aria-label={t("Tutup dialog")}><X size={17} /></button></div>{description && <p className="dialog-description" id={`${id}-description`}>{t(description)}</p>}{children}{footer && <div className="dialog-footer">{footer}</div>}</div></div>;
}

export function RowLink({ title, description, icon: Icon, onClick }: { title: string; description: string; icon: LucideIcon; onClick: () => void }) {
  return <button className="row-link" onClick={onClick}><Icon size={17} /><span><strong>{t(title)}</strong><small>{t(description)}</small></span><ChevronRight size={15} /></button>;
}
