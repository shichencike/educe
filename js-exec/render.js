#!/usr/bin/env node
// Educe JS 渲染桥脚本
// 用法: node render.js <url> [wait_ms]
// 用 headless 浏览器渲染页面，把完整 HTML 输出到 stdout，由 Rust 侧解析。
//
// 依赖: puppeteer-core（npm i puppeteer-core）
// 浏览器: 本机需有 Chrome/Chromium，可用环境变量指定：
//   CHROME_PATH=/path/to/chrome 或 PUPPETEER_EXECUTABLE_PATH=...
// 无头 Chrome 另见: npx @puppeteer/browsers install chrome-headless-shell@stable

const url = process.argv[2];
const waitMs = parseInt(process.argv[3] || '4000', 10);
if (!url) {
  console.error('用法: node render.js <url> [wait_ms]');
  process.exit(2);
}

const UA =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ' +
  '(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36';

async function main() {
  const puppeteer = require('puppeteer-core');
  const browser = await puppeteer.launch({
    headless: 'new',
    executablePath:
      process.env.CHROME_PATH || process.env.PUPPETEER_EXECUTABLE_PATH || undefined,
    args: [
      '--no-sandbox',
      '--disable-setuid-sandbox',
      '--disable-dev-shm-usage',
      '--disable-gpu',
      '--lang=zh-CN',
      '--window-size=1440,900',
    ],
  });
  try {
    const page = await browser.newPage();
    await page.setUserAgent(UA);
    await page.setViewport({ width: 1440, height: 900 });
    await page.setExtraHTTPHeaders({ 'Accept-Language': 'zh-CN,zh;q=0.9' });
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 25000 }).catch(() => {});
    // 给页面 JS 留时间渲染搜索结果
    await new Promise((r) => setTimeout(r, waitMs));
    // 向下滚动，触发懒加载
    await page.evaluate(async () => {
      for (let i = 0; i < 8; i++) {
        window.scrollBy(0, 800);
        await new Promise((r) => setTimeout(r, 300));
      }
    }).catch(() => {});
    const html = await page.content();
    process.stdout.write(html);
  } finally {
    await browser.close().catch(() => {});
  }
}

main().catch((e) => {
  console.error('渲染失败: ' + (e && e.message ? e.message : e));
  process.exit(1);
});
