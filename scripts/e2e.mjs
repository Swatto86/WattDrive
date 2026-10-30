import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { remote } from 'webdriverio';

const root = await mkdtemp(join(tmpdir(), 'wattdrive-e2e-'));
const liveUpdater = process.argv.includes('--live-updater');
const port = 4461;
const args = ['--port', String(port), '--native-port', '4462'];
if (process.env.EDGEDRIVER) args.push('--native-driver', process.env.EDGEDRIVER);
const driver = spawn('tauri-driver', args, {
  env: {
    ...process.env, WATTDRIVE_TEST_DATA_DIR: root,
    ...(liveUpdater ? { WATTDRIVE_TEST_LIVE_UPDATER: '1' } : {}),
  },
  stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
});
let logs = '';
driver.stdout.on('data', data => { logs += data; });
driver.stderr.on('data', data => { logs += data; });
driver.on('error', error => { logs += error.message; });
let browser;
async function connect() {
  const session = await remote({
    hostname: '127.0.0.1', port, logLevel: 'error',
    connectionRetryCount: 0, connectionRetryTimeout: 30000,
    capabilities: {
      'tauri:options': { application: resolve('target/debug/wattdrive-desktop' + (process.platform === 'win32' ? '.exe' : '')) },
      'wdio:enforceWebDriverClassic': true,
    },
  });
  await session.$('#view-signin').waitForDisplayed({ timeout: 30000 });
  return session;
}
try {
  let ready = false;
  for (let i = 0; i < 100; i++) {
    if (driver.exitCode !== null) throw new Error(logs);
    try { ready = (await fetch(`http://127.0.0.1:${port}/status`)).ok; } catch { /* startup */ }
    if (ready) break;
    await delay(100);
  }
  assert.ok(ready, 'tauri-driver starts');
  browser = await connect();
  await browser.$('#btn-signin').click();
  await browser.waitUntil(async () => (await browser.$('#signin-error').getText()).length > 0);
  await browser.$('#btn-settings').click();
  await browser.$('#set-folder').waitForDisplayed();
  await browser.$('#set-folder').setValue('relative-path');
  await browser.$('#btn-settings-save').click();
  await browser.waitUntil(async () => (await browser.$('#settings-error').getText()).includes('absolute'));
  const folder = join(root, 'Mirror');
  await browser.$('#set-folder').setValue(folder);
  await browser.$('#set-interval').selectByAttribute('value', '300');
  await browser.$('#set-tray').click();
  await browser.$('#btn-settings-save').click();
  await browser.$('#view-signin').waitForDisplayed();
  let saved = JSON.parse(await readFile(join(root, 'settings.json'), 'utf8'));
  assert.equal(saved.syncFolder, folder);
  assert.equal(saved.pollIntervalSecs, 300);
  assert.equal(saved.closeToTray, false);
  await browser.deleteSession();
  browser = undefined;
  browser = await connect();
  await browser.$('#btn-settings').click();
  await browser.$('#set-folder').waitForDisplayed();
  assert.equal(await browser.$('#set-folder').getValue(), folder);
  assert.equal(await browser.$('#set-interval').getValue(), '300');
  // A second write exercises replace-existing semantics on Windows.
  await browser.$('#set-interval').selectByAttribute('value', '60');
  await browser.$('#btn-settings-save').click();
  await browser.$('#view-signin').waitForDisplayed();
  saved = JSON.parse(await readFile(join(root, 'settings.json'), 'utf8'));
  assert.equal(saved.pollIntervalSecs, 60);
  // Exercise the real status-event boundary with synthetic failures, never a live account.
  await browser.execute(async () => {
    await window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
      event: 'sync-status', payload: {
        state: 'offline', detail: 'Network interrupted', signedIn: true,
        appleId: 'test@example.invalid', syncFolder: 'Synthetic mirror', lastSync: null, progress: null,
        lastReport: { planned: 20, downloaded: 3, uploaded: 2, trashedLocal: 0, trashedRemote: 0,
          conflicts: 0, foldersCreated: 1, aborted: 'Network interrupted',
          failures: [{ action: 'upload', path: 'sample.txt', error: 'Request failed (https://example.invalid/upload?token=secret-probe)' }] },
      },
    });
  });
  await browser.$('#view-main').waitForDisplayed();
  assert.ok(!(await browser.$('#view-main').getText()).includes('sample.txt'));
  assert.ok((await browser.$('#btn-messages').getText()).includes('(1)'));
  await browser.$('#btn-messages').click();
  await browser.$('#messages-dialog').waitForDisplayed();
  assert.ok((await browser.$('#messages-list').getText()).includes('sample.txt'));
  assert.ok(!(await browser.$('#messages-list').getText()).includes('secret-probe'));
  assert.equal(await browser.execute(() => document.activeElement?.textContent), 'Close');
  await browser.keys('Escape');
  await browser.$('#messages-dialog').waitForDisplayed({ reverse: true });
  assert.equal(await browser.execute(() => document.activeElement?.id), 'btn-messages');
  await browser.$('#btn-messages').click();
  await browser.$('#messages-dialog button').click();
  await browser.$('#messages-dialog').waitForDisplayed({ reverse: true });
  await browser.execute(async () => {
    await window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
      event: 'sync-status', payload: {
        state: 'syncing', detail: 'Listing iCloud: 40 folders checked, 742 items found…',
        signedIn: true, appleId: 'test@example.invalid', syncFolder: 'Synthetic mirror',
        lastSync: null, lastReport: null, progress: null,
        currentFailures: [{ action: 'upload', path: 'live-failure.txt', error: 'Connection failed' }],
      },
    });
  });
  await browser.waitUntil(async () => (await browser.$('#state-detail').getText()).includes('40 folders'));
  await browser.$('#btn-messages').click();
  await browser.$('#messages-dialog').waitForDisplayed();
  assert.ok((await browser.$('#messages-list').getText()).includes('live-failure.txt'), 'failures visible before pass ends');
  await browser.keys('Escape');
  await browser.$('#messages-dialog').waitForDisplayed({ reverse: true });
  if (liveUpdater) {
    await browser.$('#btn-about').click();
    await browser.$('#btn-check-update').waitForDisplayed();
    await browser.$('#btn-check-update').click();
    await browser.waitUntil(async () => (await browser.$('#about-update').getText()).length > 0,
      { timeout: 60000, timeoutMsg: 'Live GitHub updater check must finish' });
    assert.equal(await browser.$('#about-update').getText(), 'You are on the latest version.');
    console.log('Live updater manifest fetched and decoded through the real webview/native boundary; installation disabled in isolated tests.');
  }
  await browser.execute(() => { setTimeout(() => window.__TAURI_INTERNALS__.invoke('quit_app'), 100); });
  await browser.waitUntil(async () => {
    try { await browser.getTitle(); return false; } catch { return true; }
  }, { timeout: 10000, timeoutMsg: 'Quit must close the native app' });
  console.log('WebDriver passed: boot, sign-in validation, settings persistence, message popout/privacy/keyboard, clean quit.');
} catch (error) {
  if (browser) {
    try { console.error({ url: await browser.getUrl(), sourceLength: (await browser.getPageSource()).length, windows: await browser.getWindowHandles() }); } catch { /* app exited */ }
  }
  console.error(logs);
  try {
    console.error('Isolated app startup log:', await readFile(join(root, 'wattdrive.log'), 'utf8'));
  } catch (logError) {
    console.error('Isolated app startup log unavailable:', logError.message);
  }
  if (process.env.EDGEDRIVER_LOG) {
    try {
      console.error('Edge driver log:', await readFile(process.env.EDGEDRIVER_LOG, 'utf8'));
    } catch (logError) {
      console.error('Edge driver log unavailable:', logError.message);
    }
  }
  throw error;
} finally {
  if (browser) await browser.deleteSession().catch(error => console.error('Session cleanup:', error.message));
  driver.kill();
  await delay(500);
  await rm(root, { recursive: true, force: true, maxRetries: 5 });
}
