// Native UI and durable lifecycle QA. Fixtures use the already-active scheme.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { spawn, execFile } from 'node:child_process';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

export async function verifyAutoBoost({ send, evaluate, delay, errors, shutdownViaTray=false }) {
  assert(await evaluate('Boolean(window.__TAURI__?.core?.invoke)'), 'Native Tauri required');
  const launch=JSON.parse((await readFile('artifacts/auto-boost/native-launch.json','utf8')).replace(/^\uFEFF/,''));
  const fixtureRoot=resolve('artifacts/games')+'\\';
  for(const path of [launch.gamePath,launch.secondPath]) assert(resolve(path).startsWith(fixtureRoot),'Workspace fixtures required');
  const invoke=(command,args={})=>evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const originalSettings=await invoke('get_settings');
  const originalPlan=await invoke('get_active_power_plan');
  const originalProfiles=await invoke('get_performance_profiles');
  const before=await invoke('get_registered_games');
  assert(!before.some(game=>game.auto_boost),'Disarm existing games before isolated QA');
  assert.equal((await invoke('get_unfinished_tuning_sessions')).length,0,'Resolve existing recovery before QA');
  assert(!before.some(game=>[launch.gamePath,launch.secondPath].some(path=>game.canonical_executable_path.endsWith(path))),'Fixtures already registered');
  const checks=[],children=[],games=[];
  let prepared=false;
  const check=(name,value,details={})=>{checks.push({name,passed:Boolean(value),...details});assert(value,name);};
  const wait=async(name,predicate)=>{for(let i=0;i<75;i++){const result=await predicate();if(result)return result;await delay(200);}throw new Error('Timed out: '+name);};
  const click=async(expression)=>{await evaluate(`(${expression}).click()`);await delay(180);};
  const gaming=()=>click('document.querySelectorAll(".sidebar .nav-item")[7]');
  const row=game=>`Array.from(document.querySelectorAll('.game-row')).find(row=>row.querySelector('h3')?.textContent===${JSON.stringify(game.display_name)})`;
  const historyRow=name=>`Array.from(document.querySelectorAll('.game-history tbody tr')).find(row=>row.cells[0].textContent===${JSON.stringify(name)})`;
  const auto=()=>invoke('get_auto_boost_status');
  const gameState=id=>invoke('get_game_status').then(snapshot=>snapshot.games.find(item=>item.game.id===id));
  const processCount=(game,count)=>wait('process count '+count,async()=>{const state=await gameState(game.id);return state?.process_count===count&&state.status===(count?'running':'ready');});
  const start=path=>{const child=spawn(path,['-t','127.0.0.1'],{stdio:'ignore',windowsHide:true});child.unref();children.push(child);return child;};
  const stop=async child=>{if(child.exitCode!==null||child.signalCode!==null)return;child.ref();const exited=new Promise(done=>child.once('exit',done));child.kill();await exited;};
  const windowAction=action=>promisify(execFile)('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File','scripts/control-qa-window.ps1','-AppProcessId',String(launch.appPid),'-Action',action]);
  const shot=async name=>{const result=await send('Page.captureScreenshot',{format:'png'});await writeFile(`artifacts/auto-boost/${name}.png`,Buffer.from(result.data,'base64'));};
  const report=async failure=>writeFile('artifacts/auto-boost/native-verification.json',JSON.stringify({checkedAt:new Date().toISOString(),scope:'Native UI and lifecycle; target equals already-active Windows scheme; no physical AC transition',originalGuid:originalPlan.guid,checks,errors,failure:failure??null},null,2));
  await mkdir('artifacts/auto-boost',{recursive:true});
  try {
    // Mapping to the current scheme exercises durable sessions without OS mutation.
    for(const profileId of ['gaming','balanced'])await invoke('update_profile_mapping',{profileId,guid:originalPlan.guid,acOnly:false});
    await send('Page.reload');
    await wait('UI reloaded',()=>evaluate('Boolean(document.querySelector(".sidebar"))'));
    await gaming();
    for(const [index,path] of [launch.gamePath,launch.secondPath].entries()) {
      await click('document.querySelector(".page-actions .button-primary")');
      await promisify(execFile)('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File','scripts/select-game-fixture.ps1','-AppProcessId',String(launch.appPid),'-ExecutablePath',path]);
      await wait('native picker editor',()=>evaluate('Boolean(document.querySelector("#game-name"))'));
      const name=`Auto Boost QA ${index} ñ — ${Date.now()}`;
      await evaluate(`(()=>{const input=document.querySelector('#game-name');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(name)});input.dispatchEvent(new Event('input',{bubbles:true}));const select=document.querySelector('#game-profile');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,${JSON.stringify(index?'balanced':'gaming')});select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
      await click('document.querySelector(".game-editor .setting-row .toggle")'); // opt out of fixture AC restriction
      await click('document.querySelector(".dialog-footer .button-primary")');
      await wait('registration saved',()=>evaluate('!document.querySelector(".dialog")'));
      const game=(await invoke('get_registered_games')).find(item=>item.display_name===name);
      assert(game,'Picker registration missing');games.push(game);
      check('registration defaults off '+index,!game.auto_boost&&!game.ac_only&&game.restore_on_exit);
      await processCount(game,0);
      await wait('registered row',()=>evaluate(`Boolean(${row(game)})`));
      await click(`(${row(game)}).querySelector('.game-automation .toggle')`);
      await wait('enable confirmation',()=>evaluate('Boolean(document.querySelector(".game-enable-summary"))'));
      check('enable confirmation focuses cancel '+index,await evaluate('document.activeElement===document.querySelector(".dialog-footer button")'));
      if(index===0){
        await click('document.querySelector(".dialog-footer button")');
        check('cancel keeps Auto Boost off',!(await invoke('get_registered_games')).find(item=>item.id===game.id).auto_boost);
        await click(`(${row(game)}).querySelector('.game-automation .toggle')`);
      }
      await click('document.querySelector(".dialog-footer .button-primary")');
      await wait('opt-in saved',async()=>!(await evaluate('Boolean(document.querySelector(".dialog"))'))&&(await invoke('get_registered_games')).find(item=>item.id===game.id).auto_boost);
      game.auto_boost=true;check('explicit opt-in persisted '+index,true);
    }
    const a=start(launch.gamePath),a2=start(launch.gamePath);
    await processCount(games[0],2);
    const first=await wait('first auto activation',async()=>{const state=await auto();return state.mode==='active'?state:null;});
    check('first game pins profile',first.profile_id==='gaming');
    const b=start(launch.secondPath);await processCount(games[1],1);
    await wait('second game linked',async()=>{const history=await invoke('get_gaming_sessions');return games.every(game=>history.some(session=>session.game_name===game.display_name&&session.status==='active'&&session.tuning_session_id===first.tuning_session_id));});
    check('two games share one global tuning session',(await auto()).tuning_session_id===first.tuning_session_id);
    await click('document.querySelectorAll(".sidebar .nav-item")[0]');await invoke('stop_monitoring');
    await windowAction('Minimize');await stop(a);await processCount(games[0],1);
    check('minimized paused background keeps session',(await auto()).tuning_session_id===first.tuning_session_id);
    await stop(a2);await processCount(games[0],0);
    check('one game exit keeps other game boosted',(await auto()).mode==='active'&&(await auto()).tuning_session_id===first.tuning_session_id);
    await windowAction('Restore');await gaming();
    await wait('completed and active history shown',()=>evaluate(`Boolean(${historyRow(games[0].display_name)})&&Boolean(${historyRow(games[1].display_name)})`));
    check('active history deletion disabled',await evaluate(`(${historyRow(games[1].display_name)}).querySelector('button').disabled`));
    for(const language of ['id','en','es']) {
      await evaluate(`(()=>{const select=document.querySelector('.language-switch');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,${JSON.stringify(language)});select.dispatchEvent(new Event('change',{bubbles:true}));})()`);await delay(500);
      for(const theme of ['dark','light']) {
        if(await evaluate(`document.documentElement.dataset.theme!==${JSON.stringify(theme)}`))await click('document.querySelector(".theme-switch")');
        for(const width of [1440,860]) {
          await send('Emulation.setDeviceMetricsOverride',{width,height:width===860?610:860,deviceScaleFactor:1,mobile:false});await delay(200);
          const sizes=await evaluate('({overflow:document.documentElement.scrollWidth>innerWidth||document.querySelector(".workspace-content").scrollWidth>document.querySelector(".workspace-content").clientWidth,boost:document.querySelector(".game-boost-panel h2").textContent,history:document.querySelector(".game-history h2").textContent,html:document.documentElement.lang})');
          const expectedBoost={id:'Profil otomatis aktif',en:'Automatic profile active',es:'Perfil automático activo'}[language];
          const expectedHistory={id:'Riwayat sesi game',en:'Game session history',es:'Historial de sesiones de juego'}[language];
          check(`layout ${language} ${theme} ${width}`,!sizes.overflow&&sizes.boost===expectedBoost&&sizes.history===expectedHistory&&sizes.html===language,sizes);
          await shot(`gaming-${language}-${theme}-${width}`);
        }
      }
      await click(`(${row(games[1])}).querySelector('.game-actions button')`);
      await wait('editor focus',()=>evaluate('document.activeElement?.id==="game-name"'));
      check('armed policy edits disabled '+language,await evaluate('document.querySelector("#game-profile").disabled&&Array.from(document.querySelectorAll(".game-editor .toggle")).every(toggle=>toggle.disabled)&&!document.querySelector("#game-name").disabled'));
      await evaluate('window.dispatchEvent(new KeyboardEvent("keydown",{key:"1",ctrlKey:true,bubbles:true}))');
      check('editor keyboard isolation '+language,await evaluate('Boolean(document.querySelector(".game-editor"))&&Boolean(document.querySelector(".gaming-library"))'));
      await evaluate('document.querySelector(".dialog").scrollTop=document.querySelector(".dialog").scrollHeight');await delay(100);
      check('compact editor footer reachable '+language,await evaluate('(()=>{const rect=document.querySelector(".dialog-footer .button-primary").getBoundingClientRect();return rect.top>=0&&rect.bottom<=innerHeight;})()'));
      await shot('armed-editor-'+language);await click('document.querySelector(".dialog-heading button")');
      await evaluate('document.querySelector(".workspace-content").scrollTop=document.querySelector(".workspace-content").scrollHeight');await delay(100);
      check('compact session history reachable '+language,await evaluate('(()=>{const rect=document.querySelector(".game-history table").getBoundingClientRect();return rect.top<innerHeight&&rect.bottom>0;})()'));
      await shot('history-'+language);await evaluate('document.querySelector(".workspace-content").scrollTop=0');
    }
    await click('document.querySelector(".game-boost-panel .quiet-button")');
    await wait('manual restore UI',()=>evaluate('Boolean(document.querySelector(".notice .button"))'));
    await click('document.querySelector(".notice .button")');
    await wait('manual cancellation',async()=>{const state=await auto();return state.mode==='suspended'&&state.reason==='manual_restore';});
    check('manual restore clears unfinished tuning',(await invoke('get_unfinished_tuning_sessions')).length===0);
    await delay(5500);
    check('running game does not undo manual restore',(await auto()).mode==='suspended'&&b.exitCode===null);
    await gaming();await stop(b);await processCount(games[1],0);
    await wait('cycle cleared',async()=>(await auto()).mode==='idle');
    const history=await invoke('get_gaming_sessions');const completed=history.find(item=>item.game_name===games[0].display_name);
    await wait('history completed button',()=>evaluate(`!(${historyRow(games[0].display_name)}).querySelector('button').disabled`));
    await click(`(${historyRow(games[0].display_name)}).querySelector('button')`);
    await click('document.querySelector(".dialog-footer button")');
    check('history delete cancellation preserves summary',(await invoke('get_gaming_sessions')).some(item=>item.id===completed.id));
    await click(`(${historyRow(games[0].display_name)}).querySelector('button')`);await click('document.querySelector(".dialog-footer .button-primary")');
    await wait('history deleted',async()=>!(await invoke('get_gaming_sessions')).some(item=>item.id===completed.id));
    check('deletion preserves other game history',(await invoke('get_gaming_sessions')).some(item=>item.game_name===games[1].display_name));
    const closingGame=start(launch.secondPath);await processCount(games[1],1);
    const closing=await wait('new cycle activation',async()=>{const state=await auto();return state.mode==='active'&&state.profile_id==='balanced'?state:null;});
    check('new cycle selects newly started profile',closing.tuning_session_id!==first.tuning_session_id);
    check('original Windows scheme unchanged',(await invoke('get_active_power_plan')).guid===originalPlan.guid);
    assert.equal(errors.length,0,'Native renderer errors');
    const preferencesBeforeExit={...originalSettings,close_to_tray:shutdownViaTray};
    await invoke('update_settings',{settings:preferencesBeforeExit});await send('Emulation.clearDeviceMetricsOverride');
    await writeFile('artifacts/auto-boost/shutdown-expectation.json',JSON.stringify({appPid:launch.appPid,vitePid:launch.vitePid,childPid:closingGame.pid,gameIds:games.map(game=>game.id),gameNames:games.map(game=>game.display_name),tuningSessionId:closing.tuning_session_id,previousTuningSessionId:first.tuning_session_id,originalPlan,originalSettings,preferencesBeforeExit,exitSource:shutdownViaTray?'tray_menu':'window_close',originalProfiles,originalGameIds:before.map(game=>game.id)},null,2));
    if(shutdownViaTray) {
      const trayAction=action=>promisify(execFile)('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File','scripts/control-qa-tray.ps1','-AppProcessId',String(launch.appPid),'-Action',action,'-Language',originalSettings.language],{windowsHide:true});
      await trayAction('HideOnClose');
      check('close-to-tray keeps native window hidden and live',!(await invoke('get_desktop_status')).window_visible);
      await delay(2200);
      check('hidden window retains automatic tuning owner',(await auto()).tuning_session_id===closing.tuning_session_id&&(await auto()).mode==='active');
      await trayAction('SelectQuit');
    } else await windowAction('Close');
    check('graceful exit does not terminate game',closingGame.exitCode===null&&closingGame.signalCode===null);
    // Keep the spawning Node process alive until shutdown, so its console/job
    // lifetime cannot accidentally end the fixture before the app closes.
    await promisify(execFile)('python',['scripts/verify-auto-shutdown.py']);
    check('durable shutdown and exact fixture cleanup',true);
    await stop(closingGame);
    await report();prepared=true;
    console.log(JSON.stringify({passed:checks.length,shutdownVerified:true,scope:'Native lifecycle using unchanged active power GUID'}));
  } catch(error) {
    await report(String(error));throw error;
  } finally {
    if(!prepared){
      for(const child of children)await stop(child);
      // A terminated WebView cannot service IPC; retain the manifest for exact
      // offline recovery instead of waiting on an evaluation that cannot finish.
      if(await evaluate('Boolean(window.__TAURI__?.core?.invoke)').catch(()=>false)) {
      const fixtureHistory=(await invoke('get_gaming_sessions')).filter(session=>games.some(game=>game.display_name===session.game_name));
      const ownIds=new Set(fixtureHistory.map(session=>session.tuning_session_id));
      const own=(await invoke('get_unfinished_tuning_sessions')).filter(session=>ownIds.has(session.id)&&session.previous_guid===originalPlan.guid&&session.applied_guid===originalPlan.guid);
      for(const session of own)await invoke('restore_previous_profile',{sessionId:session.id,force:false});
      for(const game of games)if((await invoke('get_registered_games')).some(item=>item.id===game.id))await invoke('remove_registered_game',{gameId:game.id});
      await wait('fixture history closed after failure',async()=>!(await invoke('get_gaming_sessions')).some(session=>games.some(game=>game.display_name===session.game_name)&&session.status==='active'));
      for(const session of (await invoke('get_gaming_sessions')).filter(session=>games.some(game=>game.display_name===session.game_name)))await invoke('delete_gaming_session',{sessionId:session.id});
      for(const profile of originalProfiles.filter(item=>['gaming','balanced'].includes(item.id)))await invoke('update_profile_mapping',{profileId:profile.id,guid:profile.scheme_guid??'',acOnly:profile.ac_only});
      await invoke('update_settings',{settings:originalSettings});await send('Emulation.clearDeviceMetricsOverride');
      }
    }
  }
}
