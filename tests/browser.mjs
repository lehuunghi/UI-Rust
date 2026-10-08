import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
const env={...process.env,APP_KEY:'a1'.repeat(32),APP_URL:'http://localhost:8080',BIND:'127.0.0.1:8080',ADMIN_EMAIL:'browser@example.test',ADMIN_PASSWORD:'Browser-fixture-pass-123'};
execFileSync('target/debug/ui-rust',['create-admin'],{env,stdio:'inherit'});
const server=spawn('target/debug/ui-rust',['serve'],{env,stdio:'inherit'});
let browser;
try{
 for(let i=0;i<100;i++){try{if((await fetch('http://localhost:8080/healthz')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await chromium.launch();const page=await browser.newPage({viewport:{width:1440,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://localhost:8080/dang-nhap');await page.locator('[name=email]').fill(env.ADMIN_EMAIL);await page.locator('[name=password]').fill(env.ADMIN_PASSWORD);await page.locator('#authform button[type=submit]').click();await page.waitForURL('**/khach-hang');await page.locator('.stat').first().waitFor();
 await page.locator('aside a[data-page="packages"]').click();await page.locator('#create').click();await page.locator('#modalform [name=name]').fill('Business Test');await page.locator('#modalform [name=price]').fill('99000');await page.locator('#modalform button[type=submit]').click();await page.locator('td',{hasText:'Business Test'}).waitFor();
 await mkdir('screenshots',{recursive:true});await page.screenshot({path:'screenshots/admin-packages.png',fullPage:true});
 await page.goto('http://localhost:8080/');await page.locator('.plan-card').waitFor();await page.screenshot({path:'screenshots/home-desktop.png',fullPage:true});
 await page.setViewportSize({width:390,height:844});if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Mobile layout overflows viewport');await page.screenshot({path:'screenshots/home-mobile.png',fullPage:true});
 if(errors.length)throw Error(errors.join('\n'));
 console.log('Browser flow passed: login, dashboard, create plan, homepage, mobile layout.');
}finally{await browser?.close();server.kill('SIGTERM')}
