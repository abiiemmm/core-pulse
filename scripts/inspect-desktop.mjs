// Inspect the native Tauri WebView2. Start Tauri with a local debugging port first.
import { mkdir, writeFile } from 'node:fs/promises';
import { readFile } from 'node:fs/promises';
import { execFile, spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { promisify } from 'node:util';
let target;
for (let attempt=0; attempt<50&&!target; attempt++) {
  try {
    const targets = await (await fetch('http://127.0.0.1:9222/json/list')).json();
    target = targets.find(item => item.type === 'page' && item.title === 'Core Pulse');
  } catch { /* WebView2 may still be starting. */ }
  if (!target) await new Promise(resolve=>setTimeout(resolve,200));
}
if (!target) throw new Error('Core Pulse native WebView was not found.');
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }); });
let id = 0;
const pending = new Map();
const errors = [];
socket.addEventListener('message', event => {
  const data = JSON.parse(String(event.data));
  if (data.method === 'Runtime.exceptionThrown') errors.push(data.params.exceptionDetails.text);
  if (data.method === 'Runtime.consoleAPICalled' && data.params.type === 'error') errors.push(data.params.args.map(arg => arg.value ?? arg.description).join(' '));
  const request = pending.get(data.id);
  if (request) { pending.delete(data.id); data.error ? request.reject(new Error(data.error.message)) : request.resolve(data.result); }
});
function send(method, params = {}) {
  return new Promise((resolve, reject) => { const requestId = ++id; pending.set(requestId, { resolve, reject }); socket.send(JSON.stringify({ id: requestId, method, params })); });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception.description);
  return result.result.value;
}
await send('Runtime.enable');
await send('Page.enable');
const output = 'artifacts/redesign';
await mkdir(output, { recursive: true });
const mode = process.argv[2] ?? 'capture';
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const click = async expression => { await evaluate(`(${expression}).click()`); await delay(180); };
const screenshot = async name => {
  await evaluate('document.querySelector(".toast:not(.toast-error) button")?.click(); document.activeElement?.blur()');
  const shot = await send('Page.captureScreenshot', { format: 'png' });
  await writeFile(`${output}/${name}.png`, Buffer.from(shot.data, 'base64'));
};
const nav = label => click(`Array.from(document.querySelectorAll('.sidebar button.nav-item')).find(button => button.textContent.trim() === ${JSON.stringify(label)})`);
const layout = () => evaluate('({ title:document.querySelector("h1")?.textContent, horizontalOverflow:document.querySelector(".workspace-content").scrollWidth > document.querySelector(".workspace-content").clientWidth, documentOverflow:document.documentElement.scrollWidth > innerWidth, width:innerWidth, height:innerHeight })');
try {
  if (mode === 'capture') {
    console.log(JSON.stringify(await evaluate('({ native: Boolean(window.__TAURI__?.core?.invoke), title: document.querySelector("h1")?.textContent, theme: document.documentElement.dataset.theme, width: innerWidth, height: innerHeight, text: document.body.innerText })')));
    const shot = await send('Page.captureScreenshot', { format: 'png' });
    await writeFile(`${output}/overview.png`, Buffer.from(shot.data, 'base64'));
  } else if (mode === 'review') {
    const native = await evaluate('Boolean(window.__TAURI__?.core?.invoke)');
    if (!native) throw new Error('Review must run in the native Tauri WebView.');
    const originalSettings = await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const results = [];
    try {
      await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 860, deviceScaleFactor: 1, mobile: false });
      await nav('Pengaturan');
      await click('Array.from(document.querySelectorAll(".theme-choice")).find(button => button.textContent.includes("Gelap"))');
      await delay(500);
      for (const label of ['Ringkasan', 'Monitor', 'Analitik', 'Profil daya', 'Pembersihan', 'Perangkat', 'Proses', 'Gaming', 'Pengaturan']) {
        await nav(label);
        results.push(await layout());
        await screenshot(`dark-${label.replaceAll(' ', '-').toLowerCase()}`);
      }
      await click('Array.from(document.querySelectorAll(".theme-choice")).find(button => button.textContent.includes("Terang"))');
      await delay(500);
      results.push({ lightTheme: await evaluate('document.documentElement.dataset.theme') });
      await screenshot('light-settings');
      await nav('Ringkasan'); await screenshot('light-overview');
      await click('document.querySelector(".page-actions button")'); await delay(1300);
      const paused1 = await evaluate('window.__TAURI__.core.invoke("get_hardware_snapshot").then(sample=>sample.recorded_at)');
      await delay(1500);
      const paused2 = await evaluate('window.__TAURI__.core.invoke("get_hardware_snapshot").then(sample=>sample.recorded_at)');
      results.push({ monitoringPaused: paused1 === paused2 });
      await click('document.querySelector(".page-actions button")'); await delay(1400);
      results.push({ monitoringResumed: await evaluate('window.__TAURI__.core.invoke("get_hardware_snapshot").then(sample=>sample.recorded_at)') !== paused2 });
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'k', code: 'KeyK', modifiers: 2, windowsVirtualKeyCode: 75 });
      await delay(200);
      results.push({ commandPaletteOpen: await evaluate('Boolean(document.querySelector(".command-dialog"))'), searchFocused: await evaluate('document.activeElement?.getAttribute("aria-label") === "Cari halaman atau tindakan"') });
      await send('Input.insertText', { text: 'pembersihan' });
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      await delay(250);
      results.push({ paletteNavigation: await evaluate('document.querySelector("h1").textContent') });
      await nav('Pengaturan');
      await click('Array.from(document.querySelectorAll("button")).find(button=>button.textContent.trim()==="Hapus riwayat")');
      results.push({ confirmationFocused: await evaluate('document.activeElement?.textContent.trim() === "Batal"') });
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
      await delay(100);
      results.push({ confirmationCancelled: await evaluate('!document.querySelector(".dialog")') });
      await send('Emulation.setDeviceMetricsOverride', { width: 860, height: 610, deviceScaleFactor: 1, mobile: false });
      for (const label of ['Ringkasan', 'Monitor', 'Analitik', 'Profil daya', 'Pembersihan', 'Perangkat', 'Proses', 'Gaming', 'Pengaturan']) {
        await nav(label); results.push(await layout());
      }
      await screenshot('compact-settings');
      await click('document.querySelector(".toolbar-left button")');
      results.push({ sidebarCollapsed: await evaluate('document.querySelector(".app-shell").classList.contains("sidebar-collapsed")') });
      await click('document.querySelector(".toolbar-left button")');
      await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 860, deviceScaleFactor: 1, mobile: false });
      await nav('Pengaturan');
      const targetTheme = originalSettings.theme === 'dark' ? 'Gelap' : originalSettings.theme === 'light' ? 'Terang' : 'Ikuti sistem';
      await click(`Array.from(document.querySelectorAll('.theme-choice')).find(button=>button.textContent.includes(${JSON.stringify(targetTheme)}))`);
      await delay(500);
      await nav('Ringkasan'); await screenshot('overview');
      await writeFile(`${output}/verification.json`, JSON.stringify({ native, results, errors }, null, 2));
      console.log(JSON.stringify({ native, results, errors }));
    } finally {
      await evaluate(`window.__TAURI__.core.invoke('update_settings',{settings:${JSON.stringify(originalSettings)}})`);
      await send('Emulation.clearDeviceMetricsOverride');
    }
  } else if (mode === 'features') {
    const originalSettings = await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const checks = [];
    const byIndex = index => click(`document.querySelectorAll('.sidebar .nav-item')[${index}]`);
    const changeLanguage = async language => {
      await evaluate(`(()=>{const select=document.querySelector('.language-switch');select.value=${JSON.stringify(language)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
      await delay(600);
    };
    const titles = { id: ['Ringkasan','Monitor','Analitik','Profil daya','Pembersihan','Perangkat','Proses','Gaming','Pengaturan'], en: ['Overview','Monitor','Analytics','Power profiles','Cleanup','Device','Processes','Gaming','Settings'], es: ['Resumen','Monitor','Analítica','Perfiles de energía','Limpieza','Dispositivo','Procesos','Juegos','Configuración'] };
    try {
      for (const language of ['id','en','es']) {
        await changeLanguage(language);
        checks.push({ language, htmlLanguage: await evaluate('document.documentElement.lang'), savedLanguage: await evaluate('window.__TAURI__.core.invoke("get_settings").then(s=>s.language)') });
        for (const size of [{width:1440,height:860}, {width:860,height:610}]) {
          await send('Emulation.setDeviceMetricsOverride',{...size,deviceScaleFactor:1,mobile:false});
          for (let index=0; index<9; index++) {
            await byIndex(index);
            const result = await layout();
            checks.push({language,...result,translatedTitle:result.title === titles[language][index]});
          }
        }
        await send('Emulation.setDeviceMetricsOverride',{width:1440,height:860,deviceScaleFactor:1,mobile:false});
        await byIndex(2); await delay(700); await screenshot(`analytics-${language}`);
      }
      await changeLanguage('en'); await byIndex(2); await delay(600);
      for (let index=0;index<5;index++) {
        await click(`document.querySelectorAll('.analytics-controls .segmented button')[${index}]`);
        await delay(600);
        const minutes = [15,60,360,1440,4320][index];
        checks.push({minutes, report: await evaluate(`window.__TAURI__.core.invoke('get_analytics_report',{minutes:${minutes}}).then(r=>({bucket:r.bucket_seconds,points:r.points.length,valid:r.points.every(p=>Number.isFinite(p.average)&&p.minimum<=p.average&&p.average<=p.maximum&&p.sample_count>0)}))`)});
      }
      for (let index=0;index<3;index++) {
        await click(`document.querySelectorAll('.analytics-chart .segmented button')[${index}]`);
        checks.push({ chartGroup: index, sensors: await evaluate('document.querySelectorAll(".sensor-toggles button").length'), description: await evaluate('document.querySelector(".analytics-chart .section-header p").textContent') });
      }
      await click('document.querySelectorAll(".analytics-chart .segmented button")[0]');
      await click('document.querySelector(".sensor-toggles button")');
      checks.push({ sensorCanHide: await evaluate('document.querySelector(".sensor-toggles button").getAttribute("aria-pressed") === "false"') });
      await click('document.querySelector(".sensor-toggles button")');
      if (await evaluate('document.documentElement.dataset.theme !== "light"')) await click('document.querySelector(".theme-switch")');
      await delay(400); checks.push({lightTheme: await evaluate('document.documentElement.dataset.theme')});
      await screenshot('analytics-en-light');
      await send('Page.reload'); await delay(1800);
      checks.push({persistedLanguage: await evaluate('document.documentElement.lang'),persistedTheme: await evaluate('document.documentElement.dataset.theme')});
      await byIndex(8); await screenshot('settings-en-light');
      await click('document.querySelector(".theme-switch")'); await delay(400);
      checks.push({darkTheme: await evaluate('document.documentElement.dataset.theme')});
      if (checks.some(check=>check.horizontalOverflow||check.documentOverflow||check.translatedTitle===false||check.report?.valid===false)||errors.length) throw new Error('Feature review failed: '+JSON.stringify({checks,errors}));
      await writeFile(`${output}/feature-verification.json`,JSON.stringify({checks,errors},null,2));
      console.log(JSON.stringify({checks,errors}));
    } finally {
      await changeLanguage(originalSettings.language ?? 'id');
      await byIndex(8);
      const themeIndex = {dark:0,light:1,system:2}[originalSettings.theme];
      await click(`document.querySelectorAll('.theme-choice')[${themeIndex}]`);
      await delay(400);
      await send('Emulation.clearDeviceMetricsOverride');
      await byIndex(2);
    }
  } else if (mode === 'cleaner') {
    // Native WebView rendering with a simulated cleanup command boundary. Actual
    // deletion safety is covered by Rust fixtures; this mode deletes no user files.
    const originalSettings = await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const checks = [];
    const directory = 'artifacts/hardening';
    await mkdir(directory, { recursive: true });
    try {
      await evaluate(`(() => {
        const native = window.__TAURI__;
        const state = window.__cleanerTest = { calls: {}, history: [], cancelled: false };
        const planId = '76f8fe6f-092a-457f-815f-d7a5a248a73f';
        state.emit = payload => native.event.emit('cleanup:progress', { plan_id: planId, processed_count: 25, total_count: 100, deleted_count: 20, skipped_count: 5, error_count: 0, recovered_bytes: 20480, status: 'running', timestamp: new Date().toISOString(), ...payload });
        state.finish = (status, emitFinal = true) => {
          const result = { id: 'ui-test-result', category: 'user_temp', estimated_bytes: 102400, recovered_bytes: status === 'completed' ? 97280 : 20480, deleted_count: status === 'completed' ? 95 : 20, skipped_count: status === 'cancelled' ? 80 : 5, error_count: status === 'partial' ? 75 : 0, finished_at: new Date().toISOString(), status };
          state.history = [result];
          const emitted = emitFinal ? state.emit({ ...result, processed_count: status === 'cancelled' ? 25 : 100 }) : Promise.resolve();
          return emitted.then(() => state.resolve(result));
        };
        window.__TAURI__ = { ...native, core: { ...native.core, invoke: (command, args) => {
          if (!['scan_cleanable_files','execute_cleanup','cancel_cleanup','get_cleaning_history'].includes(command)) return native.core.invoke(command, args);
          state.calls[command] = (state.calls[command] ?? 0) + 1;
          if (command === 'scan_cleanable_files') return Promise.resolve({ plan_id: planId, expires_at: new Date(Date.now() + 300000).toISOString(), category: 'user_temp', estimated_bytes: 102400, eligible_count: 100, skipped_count: 0, warnings: [] });
          if (command === 'get_cleaning_history') return Promise.resolve(state.history);
          if (command === 'cancel_cleanup') { state.cancelled = true; return Promise.resolve(); }
          return new Promise(resolve => { state.resolve = resolve; void state.emit({}); });
        } } };
      })()`);
      for (const scenario of [
        { language: 'id', theme: 'dark', width: 1440, height: 860, status: 'completed', heading: 'Hasil pembersihan terakhir' },
        { language: 'en', theme: 'light', width: 860, height: 610, status: 'cancelled', heading: 'Last cleanup result' },
        { language: 'es', theme: 'dark', width: 860, height: 610, status: 'partial', heading: 'Último resultado de limpieza' },
      ]) {
        await click('document.querySelectorAll(".sidebar .nav-item")[8]');
        await click(`document.querySelectorAll('.theme-choice')[${scenario.theme === 'dark' ? 0 : 1}]`);
        await evaluate(`(() => { const select = document.querySelector('.language-switch'); select.value = '${scenario.language}'; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
        await delay(400);
        await send('Emulation.setDeviceMetricsOverride', { width: scenario.width, height: scenario.height, deviceScaleFactor: 1, mobile: false });
        await click('document.querySelectorAll(".sidebar .nav-item")[4]');
        await click('document.querySelector(".cleaner-scope button")');
        await click('document.querySelector(".scan-result > .button")');
        checks.push({ scenario: scenario.status, confirmation: await evaluate('Boolean(document.querySelector(".dialog"))') });
        await click('document.querySelector(".dialog-footer .button-primary")');
        await delay(250);
        const current = await evaluate('({value: document.querySelector("progress").value, max: document.querySelector("progress").max, scanDisabled: document.querySelector(".cleaner-scope button").disabled})');
        checks.push({ scenario: scenario.status, running: current, ...await layout() });
        if (current.value !== 25 || current.max !== 100 || !current.scanDisabled) throw new Error('Cleanup progress did not update.');
        await evaluate('window.__cleanerTest.emit({plan_id: "unrelated-plan", processed_count: 99})');
        await delay(120);
        if (await evaluate('document.querySelector("progress").value') !== 25) throw new Error('Unrelated progress replaced the active scan.');
        if (scenario.status === 'cancelled') {
          await click('document.querySelector(".cancel-clean")');
          const disabled = await evaluate('document.querySelector(".cancel-clean").disabled');
          checks.push({ cancellationRequested: await evaluate('window.__cleanerTest.cancelled'), cancelDisabled: disabled });
          if (!disabled) throw new Error('Cancellation did not disable repeated requests.');
        }
        const shot = await send('Page.captureScreenshot', { format: 'png' });
        await writeFile(directory + '/cleaner-' + scenario.language + '-running.png', Buffer.from(shot.data, 'base64'));
        await evaluate(`window.__cleanerTest.finish('${scenario.status}', ${scenario.status !== 'completed'})`);
        await delay(300);
        const terminal = await evaluate('({heading: document.querySelector(".cleanup-progress strong").textContent, status: document.querySelector(".cleanup-progress .badge").textContent, value: document.querySelector("progress").value, history: document.querySelectorAll(".table-surface tbody tr").length, scanEnabled: !document.querySelector(".cleaner-scope button").disabled})');
        checks.push({ scenario: scenario.status, terminal, ...await layout() });
        if (terminal.heading !== scenario.heading || terminal.history !== 1 || !terminal.scanEnabled || terminal.value !== (scenario.status === 'cancelled' ? 25 : 100)) throw new Error('Cleanup result was not rendered accurately.');
        await evaluate('window.__cleanerTest.emit({processed_count: 25, timestamp: new Date().toISOString()})');
        await delay(120);
        if (await evaluate('document.querySelector("progress").value') !== terminal.value) throw new Error('A late running event overwrote the terminal outcome.');
      }
      const calls = await evaluate('window.__cleanerTest.calls');
      if (checks.some(check => check.horizontalOverflow || check.documentOverflow || check.confirmation === false) || errors.length) throw new Error('Cleanup UI review failed: ' + JSON.stringify({ checks, errors }));
      const report = { nativeWebView: true, cleanupBoundary: 'simulated; no user files deleted', checks, calls, errors };
      await writeFile(directory + '/cleaner-ui-verification.json', JSON.stringify(report, null, 2));
      console.log(JSON.stringify(report));
    } finally {
      await evaluate(`window.__TAURI__.core.invoke('update_settings', {settings: ${JSON.stringify(originalSettings)}})`);
      await send('Emulation.clearDeviceMetricsOverride');
      await send('Page.reload');
    }
  } else if (mode === 'stability') {
    const directory = 'artifacts/stability';
    await mkdir(directory, { recursive: true });
    const originalSettings = await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const checks = [];
    const setQuery = value => evaluate(`(()=>{const input=document.querySelector('.process-search input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(value)});input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    try {
      await click('document.querySelectorAll(".sidebar .nav-item")[6]');
      await delay(2500);
      const sample = await evaluate(`window.__TAURI__.core.invoke('get_process_snapshot').then(s=>({total:s.total_count,rows:s.processes.length,truncated:s.truncated,readable:s.processes.filter(p=>p.executable).length,cpuMeasured:s.processes.filter(p=>p.cpu_percent!==null).length,valid:s.processes.every(p=>p.cpu_percent===null||Number.isFinite(p.cpu_percent)&&p.cpu_percent>=0&&p.cpu_percent<=100)}))`);
      checks.push({ processSample: sample });
      if (!sample.rows || !sample.cpuMeasured || !sample.valid || sample.rows > 2048) throw new Error('Native process readings invalid.');
      // Freeze displayed readings before checking sort, search, and pagination.
      await click('document.querySelectorAll(".page-actions button")[1]');
      await delay(300);
      const pausedAt = await evaluate('document.querySelector(".process-pagination > span").textContent');
      await delay(2400);
      checks.push({ processPause: pausedAt === await evaluate('document.querySelector(".process-pagination > span").textContent') });
      await click('document.querySelectorAll(".process-sort")[1]');
      const pids = await evaluate('Array.from(document.querySelectorAll(".process-table tbody tr")).map(row=>Number(row.children[1].textContent))');
      checks.push({ pidAscending: pids.every((pid,index)=>index===0||pid>=pids[index-1]) });
      await setQuery('core-pulse'); await delay(150);
      checks.push({ processSearch: await evaluate('document.querySelectorAll(".process-table tbody tr").length > 0 && Array.from(document.querySelectorAll(".process-table td:first-child strong")).every(el=>el.textContent.toLowerCase().includes("core-pulse"))') });
      await setQuery('no-such-process-core-pulse-test'); await delay(150);
      checks.push({ emptySearch: await evaluate('document.querySelectorAll(".process-table tbody tr").length === 0') });
      await setQuery(''); await delay(150);
      if (sample.rows > 50) {
        await click('document.querySelector(".process-pagination > div button:last-child")');
        checks.push({ pagination: await evaluate('document.querySelector(".process-pagination > div span").textContent.startsWith("2 / ")') });
      }
      for (const scenario of [{language:'id',theme:'dark',width:1440,height:860},{language:'en',theme:'light',width:860,height:610},{language:'es',theme:'dark',width:860,height:610}]) {
        await evaluate(`(()=>{const select=document.querySelector('.language-switch');select.value='${scenario.language}';select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
        await delay(600);
        const current = await evaluate('document.documentElement.dataset.theme');
        if (current !== scenario.theme) { await click('document.querySelector(".theme-switch")'); await delay(500); }
        await send('Emulation.setDeviceMetricsOverride',{width:scenario.width,height:scenario.height,deviceScaleFactor:1,mobile:false});
        const state = await layout();
        const paginationVisible = await evaluate('document.querySelector(".process-pagination").getBoundingClientRect().bottom <= document.querySelector(".workspace-content").getBoundingClientRect().bottom');
        checks.push({ processLanguage:scenario.language, paginationVisible, ...state });
        await evaluate('document.querySelector(".toast:not(.toast-error) button")?.click(); document.activeElement?.blur()');
        const shot = await send('Page.captureScreenshot',{format:'png'});
        await writeFile(`${directory}/processes-${scenario.language}-${scenario.theme}.png`,Buffer.from(shot.data,'base64'));
      }
      const persisted = await evaluate(`window.__TAURI__.core.invoke('get_monitoring_persistence')`);
      checks.push({ persistence: {status:persisted.status,hasSavedSample:Boolean(persisted.last_saved_at),dropped:persisted.dropped_samples} });
      await evaluate(`window.__TAURI__.event.emit('monitoring:persistence',{status:'degraded',message:'Riwayat gagal disimpan. Monitoring tetap berjalan.',last_saved_at:null,dropped_samples:2,updated_at:new Date(Date.now()+60000).toISOString()})`);
      await delay(150);
      checks.push({ persistenceNotice: await evaluate('Boolean(document.querySelector(".storage-notice"))') });
      const shot = await send('Page.captureScreenshot',{format:'png'});
      await writeFile(`${directory}/persistence-es-degraded.png`,Buffer.from(shot.data,'base64'));
      await evaluate(`window.__TAURI__.event.emit('monitoring:persistence',{status:'healthy',message:null,last_saved_at:new Date().toISOString(),dropped_samples:2,updated_at:new Date(Date.now()+61000).toISOString()})`);
      await delay(150);
      checks.push({ persistenceNoticeRecovers: await evaluate('!document.querySelector(".storage-notice")') });
      if (checks.some(c=>c.processPause===false||c.pidAscending===false||c.processSearch===false||c.emptySearch===false||c.pagination===false||c.paginationVisible===false||c.persistenceNotice===false||c.persistenceNoticeRecovers===false||c.horizontalOverflow||c.documentOverflow)||errors.length) throw new Error('Stability review failed: '+JSON.stringify({checks,errors}));
      const report={nativeWebView:true,processReadings:'real Windows read-only snapshots',persistenceNotice:'simulated status events; storage failures covered by isolated Rust fixtures',checks,errors};
      await writeFile(`${directory}/native-verification.json`,JSON.stringify(report,null,2));
      console.log(JSON.stringify(report));
    } finally {
      await evaluate(`window.__TAURI__.core.invoke('update_settings',{settings:${JSON.stringify(originalSettings)}})`);
      await send('Emulation.clearDeviceMetricsOverride');
      await send('Page.reload');
    }
  } else if (mode === 'sensors') {
    const directory = 'artifacts/sensors'; await mkdir(directory,{recursive:true});
    const invoke = (command,args={}) => evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
    const original = await invoke('get_settings');
    const checks = [];
    const waitFor = async (condition, limit=20000) => { const began=Date.now(); while(Date.now()-began<limit) { const value=await condition(); if(value) return value; await delay(100); } throw new Error('Sensor condition timed out.'); };
    try {
      await invoke('update_settings',{settings:{...original,refresh_seconds:1,gpu_adapter_id:null}});
      await evaluate('location.reload()'); await delay(1000);
      const inventory = await waitFor(async()=>{const s=await invoke('get_sensor_inventory');return s.status==='ready'&&s.devices.some(d=>d.kind==='gpu')?s:null;});
      await waitFor(async()=>{const s=await invoke('get_hardware_snapshot');return s.gpu_usage.value!==null&&s.gpu_temperature.value!==null?s:null;});
      const sample = await invoke('get_hardware_snapshot');
      if(sample.cpu_temperature.value!==null) throw new Error('Unverified CPU temperature became a measurement.');
      checks.push({nativeOrigin:target.url,inventory,initialSample:sample});
      const invalid = await evaluate(`window.__TAURI__.core.invoke('update_settings',{settings:${JSON.stringify({...original,gpu_adapter_id:'/gpu/../invalid'})}}).then(()=>false,()=>true)`);
      if(!invalid) throw new Error('Invalid adapter identity accepted.');
      checks.push({invalidAdapterRejected:invalid});
      for(const adapter of inventory.devices.filter(d=>d.kind==='gpu')) {
        await invoke('update_settings',{settings:{...original,refresh_seconds:1,gpu_adapter_id:adapter.id}});
        await evaluate('location.reload()'); await delay(1000);
        const selected = await waitFor(async()=>{const s=await invoke('get_hardware_snapshot');return s.gpu_usage.source.endsWith(adapter.id)&&s.gpu_usage.value!==null?s:null;});
        if(selected.gpu_name!==adapter.name) throw new Error('Wrong GPU name after selection.');
        const report = await invoke('get_analytics_report',{minutes:15});
        checks.push({selectedAdapter:adapter.id,gpuName:selected.gpu_name,gpuUsage:selected.gpu_usage,gpuTemperature:selected.gpu_temperature,analyticsGpuSamples:report.points.filter(p=>p.sensor_key.startsWith('gpu_')).reduce((sum,p)=>sum+p.sample_count,0)});
      }
      await invoke('update_settings',{settings:{...original,refresh_seconds:1,gpu_adapter_id:null}});
      await evaluate('location.reload()'); await delay(1500);
      const before = await invoke('get_sensor_inventory');
      const beforeSample = await invoke('get_hardware_snapshot');
      const launch = JSON.parse(await readFile(directory+'/launch.json','utf8'));
      const interruption = await promisify(execFile)('powershell',['-NoProfile','-File','scripts/interrupt-sensor.ps1','-AppPid',String(launch.pid)],{windowsHide:true,timeout:10000});
      const degraded = await waitFor(async()=>{const s=await invoke('get_sensor_inventory');return s.status==='degraded'&&s.restart_count>before.restart_count?s:null;},10000);
      const failedSample = await invoke('get_hardware_snapshot');
      if(failedSample.gpu_usage.value!==null) throw new Error('GPU retained a value after provider failure.');
      const recovered = await waitFor(async()=>{const s=await invoke('get_sensor_inventory');return s.status==='ready'&&s.restart_count>before.restart_count?s:null;});
      const afterSample = await waitFor(async()=>{const s=await invoke('get_hardware_snapshot');return s.gpu_usage.value!==null?s:null;});
      if(afterSample.recorded_at<=beforeSample.recorded_at||afterSample.cpu_usage.value===null) throw new Error('Basic monitoring did not continue through sensor restart.');
      checks.push({interruption:JSON.parse(interruption.stdout),degraded,failedGpu:failedSample.gpu_usage,recovered,basicMonitoringContinued:true});
      await click('document.querySelectorAll(".sidebar .nav-item")[1]');
      await click('document.querySelectorAll(".monitor-tabs button")[1]');
      await click('document.querySelector(".page-actions button")'); await delay(12000);
      const paused1 = await invoke('get_hardware_snapshot'); await delay(1200); const paused2 = await invoke('get_hardware_snapshot');
      if(paused1.recorded_at!==paused2.recorded_at||paused2.gpu_usage.value!==null) throw new Error('Pause or stale-value handling failed.');
      const other = inventory.devices.find(d=>d.kind==='gpu'&&!paused2.gpu_usage.source.endsWith(d.id));
      if(other) {
        await evaluate(`(()=>{const select=document.querySelector('#gpu-adapter');select.value=${JSON.stringify(other.id)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
        await waitFor(async()=>await evaluate(`document.querySelector('.sensor-heading h2').textContent === ${JSON.stringify(other.name)}`),5000);
        const pausedSelection=await invoke('get_hardware_snapshot');
        if(pausedSelection.gpu_name!==other.name||pausedSelection.gpu_usage.value!==null||pausedSelection.recorded_at!==paused2.recorded_at) throw new Error('Changing GPU while paused retained the previous adapter.');
        checks.push({adapterSelectionWhilePaused:true,adapter:other.id});
      }
      await click('document.querySelector(".page-actions button")');
      await waitFor(async()=>{const s=await invoke('get_hardware_snapshot');return s.gpu_usage.value!==null&&s.recorded_at!==paused2.recorded_at?s:null;});
      checks.push({pauseAndResume:true,staleGpuUnavailable:true});
      for(const scenario of [{language:'id',theme:'dark',width:1440,height:860},{language:'en',theme:'light',width:860,height:610},{language:'es',theme:'dark',width:1440,height:860}]) {
        await invoke('update_settings',{settings:{...original,language:scenario.language,theme:scenario.theme,refresh_seconds:1,gpu_adapter_id:null}});
        await evaluate('location.reload()'); await delay(1300);
        await send('Emulation.setDeviceMetricsOverride',{width:scenario.width,height:scenario.height,deviceScaleFactor:1,mobile:false});
        await click('document.querySelectorAll(".sidebar .nav-item")[1]');
        await click('document.querySelectorAll(".monitor-tabs button")[1]');
        const current = await layout();
        if(current.horizontalOverflow||current.documentOverflow) throw new Error('Sensor panel overflows.');
        const options = await evaluate('Array.from(document.querySelector("#gpu-adapter").options).map(o=>o.textContent)');
        if(options.length!==inventory.devices.filter(d=>d.kind==='gpu').length+1) throw new Error('GPU selector options missing.');
        const shot=await send('Page.captureScreenshot',{format:'png'});await writeFile(directory+`/monitor-${scenario.language}-${scenario.theme}.png`,Buffer.from(shot.data,'base64'));
        checks.push({scenario,layout:current,options});
      }
      const report={checkedAt:new Date().toISOString(),checks,errors};
      await writeFile(directory+'/native-verification.json',JSON.stringify(report,null,2));
      console.log(JSON.stringify(report));
    } finally {
      await invoke('update_settings',{settings:original});
      await invoke('start_monitoring');
      await send('Emulation.clearDeviceMetricsOverride');
      await evaluate('location.reload()');
    }
  } else if (mode === 'theme') {
    const settings = await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const results = [];
    await click('document.querySelectorAll(".sidebar .nav-item")[8]');
    try {
      await click('document.querySelectorAll(".theme-choice")[2]');
      for (const value of ['light','dark']) {
        await send('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value}]});
        await delay(250);
        results.push({expected:value,actual:await evaluate('document.documentElement.dataset.theme'),toolbar:await evaluate('document.querySelector(".theme-switch").textContent')});
      }
      if (results.some(result=>result.expected!==result.actual)) throw new Error('System theme did not update.');
      console.log(JSON.stringify({results,errors}));
    } finally {
      await send('Emulation.setEmulatedMedia',{features:[]});
      await click(`document.querySelectorAll('.theme-choice')[${{dark:0,light:1,system:2}[settings.theme]}]`);
      await click('document.querySelectorAll(".sidebar .nav-item")[2]');
    }
  } else if (mode === 'games') {
    if (!await evaluate('Boolean(window.__TAURI__?.core?.invoke)')) throw new Error('Gaming QA requires native Tauri.');
    const launch=JSON.parse((await readFile('artifacts/games/native-launch.json','utf8')).replace(/^\uFEFF/,''));
    if (!resolve(launch.gamePath).startsWith(resolve('artifacts/games')+'\\')) throw new Error('Executable must be a workspace game fixture.');
    const original=await evaluate('window.__TAURI__.core.invoke("get_settings")');
    const originalPlan=await evaluate('window.__TAURI__.core.invoke("get_active_power_plan")');
    const before=await evaluate('window.__TAURI__.core.invoke("get_registered_games")');
    const children=[]; const checks=[]; let gameId;
    const invoke=(command,args={})=>evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
    const picker=async(cancel=false)=>promisify(execFile)('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File','scripts/select-game-fixture.ps1','-AppProcessId',String(launch.appPid),...(cancel?['-Cancel']:['-ExecutablePath',launch.gamePath])]);
    const waitFor=async(expression)=>{for(let attempt=0;attempt<30;attempt++){if(await evaluate(expression))return;await delay(200);}throw new Error('Gaming QA condition timed out: '+expression);};
    const gameState=()=>invoke('get_game_status').then(snapshot=>snapshot.games.find(row=>row.game.id===gameId));
    const waitCount=async(expected)=>{for(let attempt=0;attempt<25;attempt++){if((await gameState())?.process_count===expected)return;await delay(200);}throw new Error('Game process count did not become '+expected);};
    try {
      await click('document.querySelectorAll(".sidebar .nav-item")[7]');
      await waitFor('document.querySelector(".gaming-library") && !document.body.innerText.includes("Memuat daftar game")');
      await click('document.querySelector(".page-actions .button-primary")');await picker(true);await delay(250);
      checks.push({pickerCancellation:!await evaluate('Boolean(document.querySelector(".game-editor"))')});
      await click('document.querySelector(".page-actions .button-primary")');await picker();
      await waitFor('Boolean(document.querySelector("#game-name"))');
      const name='Core Pulse QA ñ — '+Date.now();
      await evaluate(`{ const input=document.querySelector('#game-name');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(name)});input.dispatchEvent(new Event('input',{bubbles:true})); }`);
      await delay(150);await click('document.querySelector(".dialog-footer .button-primary")');
      await waitFor('!document.querySelector(".game-editor")');
      const registered=await invoke('get_registered_games');const game=registered.find(item=>item.display_name===name);if(!game)throw new Error('Native registration did not persist.');gameId=game.id;
      checks.push({nativePickerRegistration:true,autoBoostOff:game.auto_boost===false});
      const rejected=await evaluate(`window.__TAURI__.core.invoke('register_game',{selectionId:'not-issued-token',path:${JSON.stringify(launch.gamePath)},settings:${JSON.stringify({display_name:'invalid',profile_id:'gaming',ac_only:true,restore_on_exit:true})}}).then(()=>false,()=>true)`);
      checks.push({arbitraryPathWithoutTokenRejected:rejected});
      for(let index=0;index<2;index++)children.push(spawn(launch.gamePath,['-n','30','127.0.0.1'],{stdio:'ignore',windowsHide:true}));
      await waitCount(2);checks.push({verifiedOverlap:true});children[0].kill();await waitCount(1);checks.push({oneExitKeepsOtherProcess:true});children[1].kill();await waitCount(0);checks.push({finalExitDetected:true});
      for(const language of ['id','en','es']){
        await evaluate(`{const select=document.querySelector('.language-switch');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,'${language}');select.dispatchEvent(new Event('change',{bubbles:true}));}`);await delay(550);
        for(const theme of ['dark','light']){
          if(await evaluate(`document.documentElement.dataset.theme!=='${theme}'`))await click('document.querySelector(".theme-switch")');await delay(200);
          for(const size of [{width:1440,height:860},{width:860,height:610}]){
            await send('Emulation.setDeviceMetricsOverride',{...size,deviceScaleFactor:1,mobile:false});
            await delay(100);
            const navigationVisible=await evaluate('(()=>{const item=document.querySelector(".sidebar nav [aria-current=page]").getBoundingClientRect();const nav=document.querySelector(".sidebar nav").getBoundingClientRect();return item.top>=nav.top&&item.bottom<=nav.bottom;})()');
            const result=await layout();checks.push({language,theme,...result,navigationVisible,translatedTitle:result.title===(language==='es'?'Juegos':'Gaming')});
            await screenshot(`gaming-${language}-${theme}-${size.width}`);
          }
        }
        await click('document.querySelector(".game-actions button")');
        await waitFor('document.activeElement?.id==="game-name"');
        await evaluate('window.dispatchEvent(new KeyboardEvent("keydown",{key:"1",ctrlKey:true,bubbles:true}))');
        checks.push({language,dialogShortcutIsolation:await evaluate('Boolean(document.querySelector(".game-editor")) && document.querySelector("h1").textContent==='+JSON.stringify(language==='es'?'Juegos':'Gaming'))});
        await screenshot(`gaming-editor-${language}`);await click('document.querySelector(".dialog-heading button")');
      }
      const child=spawn(launch.gamePath,['-n','30','127.0.0.1'],{stdio:'ignore',windowsHide:true});children.push(child);await waitCount(1);
      await click('document.querySelectorAll(".game-actions button")[1]');await click('document.querySelector(".dialog-footer button")');
      checks.push({removeCancellationKeepsRegistration:(await invoke('get_registered_games')).some(item=>item.id===gameId)});
      await click('document.querySelectorAll(".game-actions button")[1]');await click('document.querySelector(".dialog-footer .button-primary")');
      await waitFor('!document.querySelector(".dialog-backdrop")');
      checks.push({removePreservesRunningProcess:child.exitCode===null && child.signalCode===null,removedFromLibrary:!(await invoke('get_registered_games')).some(item=>item.id===gameId)});
      checks.push({powerSchemeUnchanged:(await invoke('get_active_power_plan')).guid===originalPlan.guid});
      await mkdir('artifacts/games',{recursive:true});await writeFile('artifacts/games/native-verification.json',JSON.stringify({checks,errors},null,2));
      if(checks.some(check=>check.horizontalOverflow||check.documentOverflow||Object.entries(check).some(([key,value])=>value===false&&!['horizontalOverflow','documentOverflow'].includes(key))))throw new Error('Gaming QA checks failed.');
      if(errors.length)throw new Error('Native Gaming renderer errors: '+errors.join('; '));
      await mkdir('artifacts/games',{recursive:true});await writeFile('artifacts/games/native-verification.json',JSON.stringify({checks,errors},null,2));console.log(JSON.stringify({passed:checks.length,errors}));
    }finally{
      children.forEach(child=>{if(child.exitCode===null)child.kill();});
      if(gameId&&(await invoke('get_registered_games')).some(item=>item.id===gameId))await invoke('remove_registered_game',{gameId});
      const after=await invoke('get_registered_games');if(JSON.stringify(after.map(game=>game.id))!==JSON.stringify(before.map(game=>game.id)))throw new Error('Fixture cleanup changed unrelated game registrations.');
      await invoke('update_settings',{settings:original});await send('Emulation.clearDeviceMetricsOverride');await send('Page.reload');
    }
  } else if (mode === 'eval') {
    console.log(JSON.stringify(await evaluate(process.argv[3])));
  }
  console.log(JSON.stringify({ errors }));
} finally { socket.close(); }
