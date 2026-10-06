# Core Pulse — desain aplikasi desktop

Core Pulse memakai bahasa visual monokrom untuk membaca kondisi perangkat, mengatur profil daya, dan membersihkan file sementara. Susunan halaman mengikuti kebutuhan tiap pekerjaan.

## Bingkai aplikasi

- Bingkai jendela native Windows menyediakan minimize, maximize, dan close.
- Sidebar berkelompok dapat diciutkan. Toolbar menunjukkan halaman dan kondisi monitoring; statusbar menunjukkan kesegaran data.
- Hanya area kerja yang bergulir. Navigasi dan status tetap terlihat.
- Ukuran jendela kecil mengubah jumlah kolom tanpa memotong kontrol.

## Bahasa visual

- Gelap: latar `#151515`, sidebar `#101010`, permukaan `#191919`, batas `#303030`, teks `#ebebeb`.
- Terang: latar `#f7f7f7`, sidebar `#eeeeee`, permukaan putih, batas `#d6d6d6`, teks `#202020`.
- Segoe UI Variable / Segoe UI untuk antarmuka; Cascadia Mono / Consolas untuk angka dan waktu.
- Hierarki menggunakan ukuran teks, ruang, garis pembatas, dan kontras. Radius kecil; tanpa gradient atau glow.
- Status memakai teks dan ikon. Grafik CPU solid, GPU putus panjang, dan memori putus pendek pada skala yang sama.
- Pembacaan kosong tetap kosong; UI tidak membuat data pengganti.

## Susunan halaman

| Halaman | Susunan |
| --- | --- |
| Ringkasan | Identitas perangkat, strip metrik, aktivitas hardware, profil daya, pembacaan tambahan |
| Monitor | Pilihan sensor, pembacaan besar, grafik, statistik, sumber data |
| Analitik | Rentang 15 menit–3 hari, ringkasan periode, grafik penggunaan/suhu/jaringan, filter sensor, statistik berbobot, salin CSV |
| Profil daya | Skema aktif, daftar profil dan editor pemetaan, skema Windows |
| Pembersihan | Tahapan kerja, cakupan pemindaian, hasil dan masa berlaku rencana, riwayat |
| Perangkat | Identitas, spesifikasi, kapasitas penyimpanan, filter kemampuan sensor |
| Pengaturan | Preview tema, monitoring, penyimpanan data, privasi, pintasan |

## Interaksi

- `Ctrl+K`: cari halaman atau perintah, pilih dengan panah dan Enter.
- `Ctrl+1` sampai `Ctrl+7`: berpindah halaman.
- Dialog menjaga fokus keyboard, menutup dengan Escape, dan mengembalikan fokus sebelumnya.
- Pembersihan, penghapusan riwayat, dan pemulihan paksa membutuhkan konfirmasi di dalam aplikasi.
- Jeda monitoring menghentikan sampling native. Tema sistem mengikuti Windows.
- Toolbar menyediakan pergantian tema dan bahasa. Indonesia, Inggris, dan Spanyol tersedia di seluruh antarmuka; pilihan tersimpan di SQLite. Format angka dan tanggal mengikuti bahasa.
- Analitik membaca sampel asli dan agregat tersimpan. Grafik memakai interval waktu berjarak sama; periode tanpa data ditampilkan sebagai celah. Minimum dan maksimum tetap memakai nilai pengukuran asli, sedangkan rata-rata memperhitungkan jumlah sampel.

## Implementasi dan verifikasi

`src/App.tsx` membentuk bingkai aplikasi; `src/views.tsx` berisi halaman; `src/ui.tsx` berisi komponen; `src/usePulse.ts` mengatur state dan panggilan native. Token dan tata letak berada di `src/styles.css`. Ikon dibuat oleh `scripts/make_icons.py`.

`src/Analytics.tsx` dan `src/analytics-data.ts` menangani analitik. `src/i18n.ts` dan `src/locales.json` menangani terjemahan. `get_analytics_report` menggabungkan sampel mentah dan ringkasan riwayat tanpa membuat pengukuran pengganti. CSV berisi data per interval, bukan daftar sampel asli.

Build TypeScript dan Vite diverifikasi. Pemeriksaan berjalan di WebView2 aplikasi Tauri melalui `scripts/inspect-desktop.mjs`: seluruh halaman pada 1440 × 860 dan 860 × 610, tema terang, jeda dan lanjut monitoring, pencarian keyboard, pembatalan dialog, serta sidebar. Screenshot dan hasil disimpan di `artifacts/redesign/`.

Mode `features` memeriksa ketiga bahasa pada dua ukuran jendela, lima rentang analitik, tiga kelompok grafik, filter sensor, tema, serta preferensi setelah reload. `scripts/check-analytics.mjs` memeriksa pembobotan, data kosong, nilai nol, CSV, dan placeholder terjemahan. Tes Rust memeriksa gabungan data mentah/agregat dan kompatibilitas preferensi lama.
