import { Info } from 'lucide-react';
import { t } from './i18n';
import type { PulseModel } from './usePulse';

export default function SensorPanel({ model: m }: { model: PulseModel }) {
  const inventory = m.sensorInventory;
  const adapters = inventory.devices.filter(device => device.kind === 'gpu');
  const selected = m.settings.gpu_adapter_id;
  const missing = selected && !adapters.some(device => device.id === selected);
  const active = adapters.find(device => device.id === m.gpuSourceKey);
  return <div className="sensor-provider">
    <div className="sensor-provider-choice"><label className="field-label" htmlFor="gpu-adapter">{t('GPU untuk monitoring')}</label>
      <select id="gpu-adapter" value={selected ?? ''} disabled={Boolean(m.busy)} onChange={event => void m.saveSettings({ ...m.settings, gpu_adapter_id: event.target.value || null })}>
        <option value="">{t('Otomatis')}{active && !selected ? ` · ${active.name}` : ''}</option>
        {missing && <option value={selected}>{t('Adapter tersimpan belum tersedia')}</option>}
        {adapters.map(device => <option key={device.id} value={device.id}>{device.name}</option>)}
      </select>
      <p className="field-help">{t('Grafik dan analitik GPU mengikuti adapter ini.')}</p>
    </div>
    <div className="sensor-provider-status" role="status"><Info size={16} /><div>
      <strong>{t(({ waiting: 'Menunggu sensor', ready: 'Provider sensor aktif', degraded: 'Provider sensor terganggu', demo: 'Data simulasi' })[inventory.status])}</strong>
      <p>{inventory.provider}{inventory.status === 'degraded' && ` · ${t('Pemulihan otomatis berjalan')}`}</p>
      <p>{t('Suhu CPU belum tersedia sampai akses driver dapat diverifikasi.')}</p>
      <p>{t('Suhu GPU memakai inti, atau sensor terpanas bila inti tidak tersedia.')}</p>
      {missing && <p>{t('Adapter yang dipilih tidak ditemukan. Pilih adapter lain atau Otomatis.')}</p>}
    </div></div>
  </div>;
}
