'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { parseStartApps, resolveKnownFolder, isJunk, storeParts, manifestLogo, pickLogoFile } = require('../src/apps');

const env = { ProgramW6432: 'C:\\Program Files', ProgramFiles: 'C:\\Program Files', 'ProgramFiles(x86)': 'C:\\Program Files (x86)', SystemRoot: 'C:\\Windows', LOCALAPPDATA: 'C:\\Users\\t\\AppData\\Local' };

test('Start menu apps: store vs desktop, junk dropped, duplicates merged, sorted', () => {
  const apps = parseStartApps(
    {
      apps: [
        { Name: 'Tailscale', AppID: '{6D809377-6AF0-444B-8957-A3773F02200E}\\Tailscale\\tailscale-ipn.exe' },
        { Name: 'Calculator', AppID: 'Microsoft.WindowsCalculator_8wekyb3d8bbwe!App' },
        { Name: 'Uninstall Foo', AppID: '{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\\Foo\\unins000.exe' },
        { Name: 'Désinstaller Bar', AppID: 'C:\\Bar\\uninst.exe' },
        { Name: 'Foo Readme', AppID: '{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\\Foo\\readme.txt' },
        { Name: 'Foo website', AppID: 'https://foo.example' },
        { Name: 'Google Chrome', AppID: 'Chrome' },
        { Name: 'google chrome', AppID: 'Chrome.2' },
        { Name: 'Notepad', AppID: '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\notepad.exe' }
      ],
      packages: { 'Microsoft.WindowsCalculator_8wekyb3d8bbwe': 'C:\\Program Files\\WindowsApps\\Microsoft.WindowsCalculator_11.2_x64__8wekyb3d8bbwe' }
    },
    env
  );
  assert.deepEqual(apps.map((a) => a.name), ['Calculator', 'Google Chrome', 'Notepad', 'Tailscale']);
  const calc = apps[0];
  assert.equal(calc.kind, 'store');
  assert.equal(calc.storeApp, 'App');
  assert.match(calc.packageDir, /WindowsCalculator_11/);
  const ts = apps[3];
  assert.equal(ts.kind, 'desktop');
  assert.equal(ts.exe, 'C:\\Program Files\\Tailscale\\tailscale-ipn.exe');
  assert.equal(apps[2].exe, 'C:\\Windows\\System32\\notepad.exe');
  assert.equal(apps[1].exe, null); // a plain AUMID: its icon comes from the Start menu shortcut
  assert.match(ts.id, /^a[0-9a-f]{16}$/);
  // A single app comes back from ConvertTo-Json as an object, not an array.
  assert.equal(parseStartApps({ apps: { Name: 'Solo', AppID: 'Solo.App' } }, env).length, 1);
});

test('app id helpers', () => {
  assert.deepEqual(storeParts('Microsoft.XboxApp_8wekyb3d8bbwe!Microsoft.XboxApp'), { family: 'Microsoft.XboxApp_8wekyb3d8bbwe', app: 'Microsoft.XboxApp' });
  assert.equal(storeParts('Chrome'), null);
  assert.equal(storeParts('{6D809377-6AF0-444B-8957-A3773F02200E}\\a!b.exe'), null);
  assert.equal(resolveKnownFolder('{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}\\Steam\\steam.exe', env), 'C:\\Program Files (x86)\\Steam\\steam.exe');
  assert.equal(resolveKnownFolder('{00000000-0000-0000-0000-000000000000}\\x.exe', env), null);
  assert.equal(isJunk('Help and Support', 'x'), true);
  assert.equal(isJunk('Helpdesk Pro', 'x'), false);
  assert.equal(isJunk('Steam', 'C:\\x\\manual.pdf'), true);
});

test('Store app logos: manifest entry and the best scale variant', () => {
  const xml = `<Package><Applications>
    <Application Id="Other"><uap:VisualElements Square44x44Logo="Assets\\Other.png"/></Application>
    <Application Id="App" Executable="Calc.exe">
      <uap:VisualElements DisplayName="ms-resource:AppName" Square150x150Logo="Assets\\Med.png" Square44x44Logo="Assets\\AppList.png" BackgroundColor="transparent">
        <uap:DefaultTile/>
      </uap:VisualElements>
    </Application></Applications></Package>`;
  assert.equal(manifestLogo(xml, 'App'), 'Assets\\AppList.png');
  assert.equal(manifestLogo(xml, 'Missing'), 'Assets\\Other.png');
  const files = [
    'AppList.scale-100.png',
    'AppList.scale-200.png',
    'AppList.targetsize-48.png',
    'AppList.targetsize-256.png',
    'AppList.targetsize-48_altform-unplated.png',
    'AppList.targetsize-256_altform-unplated.png',
    'AppList.targetsize-256_altform-unplated_contrast-black.png',
    'AppList.targetsize-256_altform-lightunplated.png',
    'Med.scale-200.png'
  ];
  assert.equal(pickLogoFile(files, 'AppList.png'), 'AppList.targetsize-256_altform-unplated.png');
  assert.equal(pickLogoFile(['AppList.scale-100.png', 'AppList.scale-200.png'], 'AppList.png'), 'AppList.scale-200.png');
  assert.equal(pickLogoFile(['Other.png'], 'AppList.png'), null);
});
