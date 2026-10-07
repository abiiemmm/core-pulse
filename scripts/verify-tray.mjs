// Native WebView/window QA; shell callbacks and popup commands target PID-owned windows.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

export async function verifyTray({send, evaluate, delay, errors}) {
  const launch=JSON.parse((await readFile('artifacts/auto-boost/native-launch.json','utf8')).replace(/^\uFEFF/,''));
  const invoke=(command,args={})=>evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const original=await invoke('get_settings');
  const originalPlan=await invoke('get_active_power_plan');
  const checks=[];
  const check=(name,passed)=>{assert(passed,name);checks.push({name,passed:true});console.log('Tray QA: '+name);};
  const wait=async(name,test)=>{for(let i=0;i<60;i++){const result=await test();if(result)return result;await delay(200);}throw new Error('Timed out: '+name);};
  const click=async expression=>{await evaluate(`(${expression}).click()`);await delay(200);};
  const action=(action,language='id')=>promisify(execFile)('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File','scripts/control-qa-tray.ps1','-AppProcessId',String(launch.appPid),'-Action',action,'-Language',language],{windowsHide:true});
  const status=()=>invoke('get_desktop_status');
  const timestamp=async()=>(await invoke('get_hardware_snapshot')).recorded_at;
  const settingsPage=()=>click('document.querySelectorAll(".sidebar .nav-item")[8]');
  const section='document.querySelectorAll(".settings-section")[3]';
  const reload=async()=>{await send('Page.reload');await wait('settings loaded',()=>evaluate('Boolean(document.querySelector(".sidebar"))'));await settingsPage();await wait('tray available in UI',()=>evaluate(`!(${section}).querySelector('.toggle').disabled`));};
  const paused=async name=>{await delay(1800);const at=await timestamp();await delay(2200);check(name,await timestamp()===at);};
  await mkdir('artifacts/tray',{recursive:true});
  try {
    check('Windows shell tray supported',(await status()).tray_available);
    await invoke('update_settings',{settings:{...original,close_to_tray:false,monitor_in_background:false,refresh_seconds:1,language:'id'}});
    await reload();
    check('close-to-tray initially off',await evaluate(`(${section}).querySelector('.toggle').getAttribute('aria-checked')==='false'`));
    await click(`(${section}).querySelector('.toggle')`);
    await wait('tray opt-in saved',async()=>(await invoke('get_settings')).close_to_tray);
    await reload();
    check('explicit tray preference survives reload',await evaluate(`(${section}).querySelector('.toggle').getAttribute('aria-checked')==='true'`));
    let at=await timestamp();await wait('visible sampling',async()=>await timestamp()!==at);
    await action('HideOnClose');
    check('X hides live native window',!(await status()).window_visible);
    await paused('hidden sampling pauses without background opt-in');
    await action('SelectOpen');
    check('native menu opens hidden window',(await status()).window_visible&&!(await status()).window_minimized);
    at=await timestamp();await wait('sampling after reopen',async()=>await timestamp()!==at);
    check('reopening resumes sampling',true);
    await invoke('update_settings',{settings:{...(await invoke('get_settings')),monitor_in_background:true}});
    await action('HideOnClose');at=await timestamp();
    await wait('hidden background sampling',async()=>await timestamp()!==at);
    check('hidden sampling follows background opt-in',true);
    await action('Open');
    check('native tray left-click opens window',(await status()).window_visible);
    await click('document.querySelectorAll(".sidebar .nav-item")[0]');
    await click('document.querySelector(".page-actions .button")');
    await action('HideOnClose');
    await paused('explicit UI pause wins over background opt-in');
    await promisify(execFile)(resolve('src-tauri/target/debug/core-pulse.exe'),[],{windowsHide:true,timeout:5000});
    check('second launch restores existing hidden window',(await status()).window_visible);
    await click('document.querySelector(".page-actions .button")');
    for(const language of ['id','en','es']) {
      for(const theme of ['dark','light']) {
        await invoke('update_settings',{settings:{...(await invoke('get_settings')),language,theme}});
        await reload();
        await action('InspectMenu',language);
        check(`native tray menu translated ${language} ${theme}`,true);
        for(const [width,height] of [[1440,860],[860,610]]) {
          await send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
          await evaluate(`(()=>{const main=document.querySelector('.workspace-content');main.scrollTop+=(${section}).getBoundingClientRect().top-main.getBoundingClientRect().top-16;window.scrollTo(0,0);})()`);
          check(`settings layout ${language} ${theme} ${width}`,await evaluate('document.documentElement.scrollWidth<=innerWidth&&Boolean(document.querySelector(".settings-layout"))'));
          await click(`(${section}).querySelector('.button')`);
          check(`quit confirmation focuses cancel ${language} ${theme} ${width}`,await evaluate('document.activeElement===document.querySelector(".dialog-footer button")'));
          const before=await evaluate('document.querySelector(".page-header h1")?.textContent');
          await evaluate('window.dispatchEvent(new KeyboardEvent("keydown",{key:"1",ctrlKey:true,bubbles:true}))');
          check(`quit dialog isolates navigation ${language} ${theme} ${width}`,await evaluate('document.querySelector(".page-header h1")?.textContent')===before);
          check(`quit footer fits ${language} ${theme} ${width}`,await evaluate('document.querySelector(".dialog-footer").getBoundingClientRect().bottom<=innerHeight'));
          const shot=await send('Page.captureScreenshot',{format:'png'});
          await writeFile(`artifacts/tray/quit-${language}-${theme}-${width}.png`,Buffer.from(shot.data,'base64'));
          await click('document.querySelector(".dialog-footer button")');
          check(`quit cancellation retains app ${language} ${theme} ${width}`,(await status()).window_visible);
        }
      }
    }
    check('Windows power scheme unchanged',(await invoke('get_active_power_plan')).guid===originalPlan.guid);
    check('native renderer has no errors',errors.length===0);
    await writeFile('artifacts/tray/native-verification.json',JSON.stringify({checkedAt:new Date().toISOString(),scope:'Native registered icon, targeted shell callback and native popup command messages; no global pointer input',checks,errors},null,2));
    console.log(JSON.stringify({trayChecks:checks.length,errors}));
  } catch(error) {
    await writeFile('artifacts/tray/failed-verification.json',JSON.stringify({checks,errors,failure:String(error)},null,2));
    throw error;
  } finally {
    if(await evaluate('Boolean(window.__TAURI__?.core?.invoke)').catch(()=>false)) {
      await invoke('update_settings',{settings:original});
      await send('Emulation.clearDeviceMetricsOverride');
      await send('Page.reload');
    }
  }
}

export async function verifyTrayQuitUi({send, evaluate, delay, errors}) {
  const launch=JSON.parse((await readFile('artifacts/auto-boost/native-launch.json','utf8')).replace(/^\uFEFF/,''));
  const invoke=(command,args={})=>evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const original=await invoke('get_settings');
  const originalPlan=await invoke('get_active_power_plan');
  assert(!original.close_to_tray,'Use the restored QA preferences');
  const click=async expression=>{await evaluate(`(${expression}).click()`);await delay(250);};
  await click('document.querySelectorAll(".sidebar .nav-item")[8]');
  const section='document.querySelectorAll(".settings-section")[3]';
  await click(`(${section}).querySelector('.toggle')`);
  const preferencesBeforeExit=await invoke('get_settings');
  assert(preferencesBeforeExit.close_to_tray,'Tray opt-in must be active when explicit quit is requested');
  const checks=[];
  for(const [width,height] of [[1440,860],[860,610]]) {
    await send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    await evaluate(`(()=>{const main=document.querySelector('.workspace-content');main.scrollTop+=(${section}).getBoundingClientRect().top-main.getBoundingClientRect().top-16;window.scrollTo(0,0);})()`);
    await click(`(${section}).querySelector('.button')`);
    assert(await evaluate('scrollY===0&&document.activeElement===document.querySelector(".dialog-footer button")'),'Dialog must retain viewport and focus');
    const shot=await send('Page.captureScreenshot',{format:'png'});
    await writeFile(`artifacts/tray/final-quit-ui-${width}.png`,Buffer.from(shot.data,'base64'));
    checks.push({name:'final settings component and quit dialog '+width,passed:true});
    if(width===1440)await click('document.querySelector(".dialog-footer button")');
  }
  assert.equal(errors.length,0,'Native renderer errors');
  await writeFile('artifacts/tray/ui-quit-expectation.json',JSON.stringify({appPid:launch.appPid,original,preferencesBeforeExit,originalPlan,checks},null,2));
  await evaluate('document.querySelector(".dialog-footer .button-primary").click()').catch(error=>{
    if(!String(error).includes('Native WebView disconnected')&&!String(error).includes('Native WebView is closed'))throw error;
  });
  console.log('Native quit UI invoked with close-to-tray enabled; verify process exit and persisted preferences offline.');
}
