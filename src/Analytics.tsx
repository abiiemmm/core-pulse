import { useCallback, useEffect, useRef, useState } from 'react';
import { Check, ClipboardCopy, Clock3, Info, LoaderCircle, RefreshCw } from 'lucide-react';
import { api } from './api';
import { t } from './i18n';
import { ANALYTICS_SENSORS, analyticsCsv, chartMaximum, chartSeries, sensorStatistics } from './analytics-data';
import type { ChartGroup } from './analytics-data';
import type { AnalyticsReport } from './types';
import type { PulseModel } from './usePulse';
import { DataRow, EmptyState, HistoryChart, PageHeader, SectionHeader, Segmented, dateText, formatNumber } from './ui';

type Range = 15 | 60 | 360 | 1440 | 4320;
const RANGE_OPTIONS = [{ value: 15, label: '15 menit' }, { value: 60, label: '1 jam' }, { value: 360, label: '6 jam' }, { value: 1440, label: '24 jam' }, { value: 4320, label: '3 hari' }] as const;
const GROUP_OPTIONS = [{ value: 'usage', label: 'Penggunaan' }, { value: 'temperature', label: 'Suhu' }, { value: 'network', label: 'Jaringan' }] as const;
const TITLES: Record<ChartGroup, string> = { usage: 'Riwayat penggunaan', temperature: 'Riwayat suhu', network: 'Riwayat jaringan' };

export default function Analytics({ model: m }: { model: PulseModel }) {
  const [range, setRange] = useState<Range>(15);
  const [group, setGroup] = useState<ChartGroup>('usage');
  const [hidden, setHidden] = useState<string[]>([]);
  const [report, setReport] = useState<AnalyticsReport>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const version = ++request.current;
    setLoading(true);
    try {
      const next = await api.getAnalytics(range);
      if (version === request.current) { setReport(next); setError(''); }
    } catch (cause) { if (version === request.current) setError(String(cause)); }
    finally { if (version === request.current) setLoading(false); }
  }, [range, m.gpuSourceKey]);
  useEffect(() => { setReport(undefined); void refresh(); return () => { request.current += 1; }; }, [refresh]);
  useEffect(() => {
    if (!m.running) return;
    const interval = window.setInterval(() => void refresh(), 15000);
    return () => clearInterval(interval);
  }, [refresh, m.running]);

  const points = report?.points ?? [];
  const selected = ANALYTICS_SENSORS.filter(sensor => sensor.group === group);
  const visible = selected.filter(sensor => !hidden.includes(sensor.key));
  const series = report ? chartSeries(report, visible.map(sensor => sensor.key)) : { times: [], values: [] };
  const unit = group === 'temperature' ? '°C' : group === 'network' ? 'KB/s' : '%';
  const peak = Math.max(0, ...points.filter(point => visible.some(sensor => sensor.key === point.sensor_key)).map(point => point.maximum));
  const maximum = group === 'usage' ? 100 : group === 'temperature' ? Math.max(100, chartMaximum(peak)) : chartMaximum(peak);
  const stats = ANALYTICS_SENSORS.map(sensor => ({ ...sensor, ...sensorStatistics(points, sensor.key) }));
  const tiles = [
    { label: 'CPU rata rata', key: 'cpu_usage', value: 'average' },
    { label: 'Memori rata rata', key: 'ram_usage', value: 'average' },
    { label: 'Puncak CPU', key: 'cpu_usage', value: 'maximum' },
    { label: 'Unduh rata rata', key: 'network_down_kbps', value: 'average' },
  ] as const;
  async function copyCsv() {
    if (!report) return;
    try { await navigator.clipboard.writeText(analyticsCsv(report)); m.notify(t('Data analitik disalin sebagai CSV.')); }
    catch { m.notify(t('Tidak dapat menyalin data. Coba lagi.'), true); }
  }
  return <>
    <PageHeader title={t('Analitik')} description={t('Riwayat hardware, statistik, dan pola penggunaan perangkat.')} actions={<button className="button" disabled={loading} onClick={() => void refresh()}><RefreshCw size={14} className={loading ? 'spin' : ''} />{t('Muat ulang')}</button>} />
    <div className="analytics-controls"><Segmented<Range> label={t('Rentang waktu')} value={range} options={RANGE_OPTIONS.map(option => ({ ...option, label: t(option.label) }))} onChange={setRange} /><span><Clock3 size={13} />{t(m.running ? 'Pembaruan otomatis setiap 15 detik' : 'Pembaruan otomatis dijeda')}</span></div>
    {error && <div className="notice" role="alert"><Info size={17} /><div><strong>{t('Riwayat gagal dimuat')}</strong><p>{error}</p></div><button className="button" onClick={() => void refresh()} disabled={loading}>{t('Coba lagi')}</button></div>}
    <div className="analytics-summary analytics-summary-four">{tiles.map(tile => {
      const stat = stats.find(sensor => sensor.key === tile.key)!;
      return <div key={tile.label}><span>{t(tile.label)}</span><strong>{formatNumber(stat[tile.value], 1)}{stat.count > 0 && <small>{stat.unit}</small>}</strong><p>{stat.count ? t('{n} pembacaan valid', { n: formatNumber(stat.count) }) : t('Belum ada pembacaan valid')}</p></div>;
    })}</div>
    <section className="surface chart-surface analytics-chart"><SectionHeader title={t(TITLES[group])} description={report ? `${t('Rata rata per interval {n} detik', { n: report.bucket_seconds })} · ${unit}` : t('Memuat riwayat…')} action={<Segmented<ChartGroup> label={t('Kelompok grafik')} value={group} options={GROUP_OPTIONS.map(option => ({ ...option, label: t(option.label) }))} onChange={setGroup} />} />
      <div className="sensor-toggles" role="group" aria-label={t('Sensor ditampilkan')}>{selected.map(sensor => <button key={sensor.key} className={hidden.includes(sensor.key) ? '' : 'is-selected'} aria-pressed={!hidden.includes(sensor.key)} onClick={() => setHidden(previous => previous.includes(sensor.key) ? previous.filter(key => key !== sensor.key) : [...previous, sensor.key])}><i className={`legend-${sensor.plot}`} />{t(sensor.label)}{!hidden.includes(sensor.key) && <Check size={12} />}</button>)}</div>
      {loading && !report ? <div className="analytics-loading" role="status"><LoaderCircle size={22} className="spin" />{t('Memuat riwayat…')}</div> : visible.length ? <HistoryChart plots={visible.map((sensor, index) => ({ key: sensor.plot, label: t(sensor.label), values: series.values[index] }))} times={series.times} unit={unit} maximum={maximum} countLabel={t('{n} interval', { n: series.times.length })} tall /> : <EmptyState icon={Info} title={t('Pilih setidaknya satu sensor.')} description={t('Sensor ditampilkan')} />}
    </section>
    <section className="surface table-surface analytics-statistics"><SectionHeader title={t('Statistik seluruh sensor')} description={t('Rata rata berbobot; nilai minimum dan maksimum berasal dari pengukuran asli.')} action={<button className="button" disabled={!points.length || loading} onClick={() => void copyCsv()}><ClipboardCopy size={14} />{t('Salin CSV')}</button>} /><div className="table-scroll"><table><thead><tr><th>{t('Sensor')}</th><th>{t('Rata rata')}</th><th>{t('Terendah')}</th><th>{t('Tertinggi')}</th><th>{t('Pembacaan')}</th></tr></thead><tbody>{stats.map(sensor => <tr key={sensor.key}><td>{t(sensor.label)}<small className="statistic-unit">{sensor.unit}</small></td><td>{formatNumber(sensor.average, 1)}</td><td>{formatNumber(sensor.minimum, 1)}</td><td>{formatNumber(sensor.maximum, 1)}</td><td>{formatNumber(sensor.count)}</td></tr>)}</tbody></table></div></section>
    <section className="surface analytics-period"><SectionHeader title={t('Ringkasan periode')} /><div className="vitals-grid"><DataRow label={t('Interval pertama')} value={points[0] ? dateText(points[0].recorded_at) : '—'} /><DataRow label={t('Interval terakhir')} value={points.at(-1) ? dateText(points.at(-1)!.recorded_at) : '—'} /><DataRow label={t('Sensor dengan data')} value={`${stats.filter(sensor => sensor.count > 0).length} / ${stats.length}`} /><DataRow label={t('Interval grafik')} value={report ? t('{n} detik', { n: report.bucket_seconds }) : '—'} /></div>{!points.length && !loading && <p className="inline-note">{t('Belum ada riwayat pada periode ini.')}</p>}</section>
    <div className="inline-note"><Info size={15} /><span>{t('Data hanya tersedia selama monitoring berjalan. Jeda dan sensor tidak tersedia ditampilkan sebagai celah, bukan nol.')}<br />{t('Data lama menggunakan ringkasan per menit. Riwayat yang belum direkam tidak dapat ditampilkan.')}</span></div>
  </>;
}
