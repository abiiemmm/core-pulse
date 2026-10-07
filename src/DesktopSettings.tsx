import { Power } from 'lucide-react';
import { isDesktop } from './api';
import { t } from './i18n';
import { Toggle } from './ui';
import type { PulseModel } from './usePulse';

export default function DesktopSettings({ model: m }: { model: PulseModel }) {
  const { settings, busy, desktopStatus } = m;
  return <section className="settings-section">
    <div className="settings-section-label">
      <h2>{t('Jendela & system tray')}</h2>
      <p>{t('Atur perilaku saat jendela ditutup.')}</p>
    </div>
    <div className="settings-section-body">
      <div className="setting-row">
        <div>
          <strong>{t('Tutup ke system tray')}</strong>
          <p>{t('Tombol X menyembunyikan jendela. Klik ikon Core Pulse di tray untuk membukanya kembali.')}</p>
        </div>
        <Toggle label={t('Tutup ke system tray')} checked={settings.close_to_tray}
          disabled={!isDesktop || Boolean(busy) || (!desktopStatus.tray_available && !settings.close_to_tray)}
          onChange={checked => void m.saveSettings({ ...settings, close_to_tray: checked })} />
      </div>
      {!desktopStatus.tray_available && <p className="field-help">
        {t(isDesktop ? 'System tray tidak tersedia. Jendela tetap dapat ditutup untuk keluar.' : 'System tray tersedia di aplikasi desktop Windows.')}
      </p>}
      <p className="field-help">{t('Auto Boost dan perlindungan AC tetap berjalan saat jendela tersembunyi. Monitoring mengikuti pilihan latar belakang.')}</p>
      <div className="setting-row">
        <div>
          <strong>{t('Keluar sepenuhnya')}</strong>
          <p>{t('Hentikan aplikasi melalui tombol ini atau menu keluar di tray.')}</p>
        </div>
        <button className="button" disabled={!isDesktop || Boolean(busy)} onClick={() => m.setConfirmation('quit')}>
          <Power size={14} />{t('Keluar dari Core Pulse')}
        </button>
      </div>
    </div>
  </section>;
}
