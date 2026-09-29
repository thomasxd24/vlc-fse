'use strict';

/*
 * Renders the app icon (renderer/logo-mark.svg on a dark rounded tile) to every size the shells need:
 * Tauri's icons, the FSE package assets and build/icon.png, plus a PNG-in-ICO for Windows.
 *
 *   npm i --no-save playwright && npx playwright install chromium
 *   node scripts/build-icons.js
 */

const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..');
// The mark's own viewBox hugs the glyph; nest it in the middle of the tile.
const mark = fs.readFileSync(path.join(root, 'renderer', 'logo-mark.svg'), 'utf8').replace(/<svg /, '<svg x="131" y="96" width="250" height="320" ');

const tile = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <defs><linearGradient id="t" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#241a2c"/><stop offset="1" stop-color="#0b0c10"/></linearGradient></defs>
  <rect width="512" height="512" rx="116" fill="url(#t)"/>
  ${mark}
</svg>`;

// [file, width, height, kind]: 'tile' = the square icon, 'wide' = tile centred on a transparent canvas.
const outputs = [
  ['build/icon.png', 512, 512, 'tile'],
  ['src-tauri/app/icons/icon.png', 512, 512, 'tile'],
  ['src-tauri/app/icons/128x128@2x.png', 256, 256, 'tile'],
  ['src-tauri/app/icons/128x128.png', 128, 128, 'tile'],
  ['src-tauri/app/icons/32x32.png', 32, 32, 'tile'],
  ['build/fse/Assets/Square150x150Logo.png', 150, 150, 'tile'],
  ['build/fse/Assets/Square71x71Logo.png', 71, 71, 'tile'],
  ['build/fse/Assets/Square44x44Logo.png', 44, 44, 'tile'],
  ['build/fse/Assets/StoreLogo.png', 50, 50, 'tile'],
  ['build/fse/Assets/Wide310x150Logo.png', 310, 150, 'wide'],
  ['build/fse/Assets/SplashScreen.png', 620, 300, 'wide']
];
const icoSizes = [16, 24, 32, 48, 64, 128, 256];

async function render(page, w, h, kind) {
  const side = kind === 'wide' ? Math.round(h * 0.8) : Math.min(w, h);
  const svg = tile.replace('<svg ', `<svg width="${side}" height="${side}" `);
  await page.setViewportSize({ width: w, height: h });
  await page.setContent(`<body style="margin:0;background:transparent;display:grid;place-items:center;width:${w}px;height:${h}px">${svg}</body>`);
  return page.screenshot({ omitBackground: true, type: 'png' });
}

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const [file, w, h, kind] of outputs) {
    fs.writeFileSync(path.join(root, file), await render(page, w, h, kind));
    console.log('wrote', file);
  }
  const pngs = [];
  for (const s of icoSizes) pngs.push([s, await render(page, s, s, 'tile')]);
  const head = Buffer.alloc(6 + 16 * pngs.length);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(pngs.length, 4);
  let offset = head.length;
  pngs.forEach(([s, png], i) => {
    const e = 6 + 16 * i;
    head[e] = s === 256 ? 0 : s;
    head[e + 1] = s === 256 ? 0 : s;
    head.writeUInt16LE(1, e + 4);
    head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(png.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  fs.writeFileSync(path.join(root, 'src-tauri/app/icons/icon.ico'), Buffer.concat([head, ...pngs.map(([, p]) => p)]));
  console.log('wrote src-tauri/app/icons/icon.ico');
  await browser.close();
})();
