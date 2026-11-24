import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { resolve, join, extname, sep } from 'node:path'
import { readFileSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { createServer } from 'node:http'
const required = name => {if (!process.env[name]) throw Error('Missing explicit owned browser input: '+name);return process.env[name]}
const root=resolve(required('MIXYARD_CONFIRMATION_ASSETS')), executable=required('MIXYARD_CONFIRMATION_CHROMIUM')
assert.equal(JSON.parse(readFileSync(join(root,'controller-source.json'),'utf8')).format,'classic-iife')
const require=createRequire(new URL('../../apps/web/package.json',import.meta.url)),{chromium}=require('@playwright/test')
const directory=mkdtempSync(join(tmpdir(),'mixyard-confirmation-browser-'))
const server=createServer((req,res)=>{try{
 const path=resolve(root,'.'+new URL(req.url,'http://owned').pathname)
 if(!path.startsWith(root+sep))throw Error('owned asset only')
 res.setHeader('Content-Type',extname(path)==='.js'?'text/javascript':extname(path)==='.css'?'text/css':'text/html');res.end(readFileSync(path))
}catch{res.writeHead(404);res.end()}})
let context
try {
 await new Promise(ok=>server.listen(0,'127.0.0.1',ok))
 context=await chromium.launchPersistentContext(directory,{executablePath:executable,headless:true,viewport:{width:390,height:844},args:['--mute-audio','--disk-cache-size=1','--media-cache-size=1']})
 const page=await context.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.name))
 await page.goto(`http://127.0.0.1:${server.address().port}/index.html`)
 const button=name=>page.getByRole('button',{name,exact:true})
 await button('Fixture load example').waitFor()
 assert.equal(await page.getByRole('navigation',{name:'Owned fixture controls'}).getByRole('button').count(),7)
 await button('Fixture load example').click();await button('Fixture edit gain').click()
 await page.waitForFunction(()=>document.querySelector('input[aria-label="mic to Call gain"]')?.value==='0.37')
 async function answer(name,approve){
  const handled=new Promise((ok,fail)=>page.once('dialog',async dialog=>{try{assert.equal(dialog.type(),'confirm');if(approve)await dialog.accept();else await dialog.dismiss();ok()}catch(e){fail(e)}}))
  await button(name).click();await handled
 }
 await answer('Fixture load example',false);assert.equal(await page.locator('input[aria-label="mic to Call gain"]').inputValue(),'0.37')
 await answer('Fixture load example',true);assert.notEqual(await page.locator('input[aria-label="mic to Call gain"]').inputValue(),'0.37')
 await button('Fixture codecs').click();assert.equal(await page.locator('#codec-draft option').count(),1)
 await button('Fixture add codec').click();await button('Fixture edit codec').click()
 await answer('Fixture remove codec',false);assert.equal(await page.locator('#codec-draft option').count(),2)
 await answer('Fixture remove codec',true);assert.equal(await page.locator('#codec-draft option').count(),1)
 assert.deepEqual(errors,[]);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
 console.log('PASS actual classic shared App: seven controls, dirty replacement Cancel/Confirm, codec removal Cancel/Confirm,390px/no page errors. Browser evidence only; NOT native dialogs.')
} finally {
 if(context)await context.close()
 await new Promise(ok=>server.close(ok));rmSync(directory,{recursive:true,force:true})
}
