import type { AnalyticsPoint, AnalyticsReport } from './types';

export const ANALYTICS_SENSORS = [
  { key: 'cpu_usage', label: 'CPU', unit: '%', group: 'usage', plot: 'cpu' },
  { key: 'gpu_usage', label: 'GPU', unit: '%', group: 'usage', plot: 'gpu' },
  { key: 'ram_usage', label: 'Memori', unit: '%', group: 'usage', plot: 'ram' },
  { key: 'cpu_temperature', label: 'Suhu CPU', unit: '°C', group: 'temperature', plot: 'cpu' },
  { key: 'gpu_temperature', label: 'Suhu GPU', unit: '°C', group: 'temperature', plot: 'gpu' },
  { key: 'network_down_kbps', label: 'Unduh', unit: 'KB/s', group: 'network', plot: 'cpu' },
  { key: 'network_up_kbps', label: 'Unggah', unit: 'KB/s', group: 'network', plot: 'gpu' },
  { key: 'ram_used_gb', label: 'Memori terpakai', unit: 'GB', group: 'capacity', plot: 'ram' },
  { key: 'disk_used_gb', label: 'Penyimpanan', unit: 'GB', group: 'capacity', plot: 'disk' },
] as const;
export type ChartGroup = 'usage' | 'temperature' | 'network';

export function sensorStatistics(points: AnalyticsPoint[], key: string) {
  const values = points.filter(point => point.sensor_key === key && point.sample_count > 0);
  const count = values.reduce((sum, point) => sum + point.sample_count, 0);
  return {
    count,
    average: count ? values.reduce((sum, point) => sum + point.average * point.sample_count, 0) / count : null,
    minimum: values.length ? Math.min(...values.map(point => point.minimum)) : null,
    maximum: values.length ? Math.max(...values.map(point => point.maximum)) : null,
  };
}

export function chartSeries(report: AnalyticsReport, keys: string[]) {
  const step = report.bucket_seconds * 1000;
  const start = Math.floor(Date.parse(report.from) / step) * step;
  const end = Math.floor(Date.parse(report.to) / step) * step;
  const times = Array.from({ length: Math.floor((end - start) / step) + 1 }, (_, index) => new Date(start + index * step).toISOString());
  const lookup = new Map(report.points.map(point => [`${point.sensor_key}:${Date.parse(point.recorded_at)}`, point.average]));
  return { times, values: keys.map(key => times.map(time => lookup.get(`${key}:${Date.parse(time)}`) ?? null)) };
}

export function chartMaximum(value: number) {
  const magnitude = 10 ** Math.floor(Math.log10(Math.max(1, value)));
  return ([1, 2, 5, 10].find(step => step * magnitude >= value * 1.05) ?? 10) * magnitude;
}

export function analyticsCsv(report: AnalyticsReport) {
  const rows: (string | number)[][] = [
    ['interval_start', 'interval_seconds', 'sensor_key', 'unit', 'average', 'minimum', 'maximum', 'sample_count'],
    ...report.points.map(point => [point.recorded_at, report.bucket_seconds, point.sensor_key, ANALYTICS_SENSORS.find(sensor => sensor.key === point.sensor_key)?.unit ?? '', point.average, point.minimum, point.maximum, point.sample_count]),
  ];
  return rows.map(row => row.map(value => `"${String(value).replaceAll('"', '""')}"`).join(',')).join('\r\n');
}
