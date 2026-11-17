import test from 'node:test';
import assert from 'node:assert/strict';
import {validDesktopClient,checkFile} from './check-google-client.mjs';
const id='12345-desktop.apps.googleusercontent.com';
test('desktop OAuth packaging accepts both supported developer formats',()=>{
  for(const value of [{clientId:id,clientSecret:'FAKE_ONLY'},{installed:{client_id:id,client_secret:'FAKE_ONLY',redirect_uris:['http://localhost']}}])assert.equal(validDesktopClient(JSON.stringify(value)),true);
});
test('missing, web-only and malformed credentials cannot produce a Google-ready build',()=>{
  for(const value of [null,[],{}, {web:{client_id:id,client_secret:'FAKE_ONLY'}},{clientId:'https://evil.invalid',clientSecret:'FAKE_ONLY'},{clientId:id,clientSecret:''},{clientId:id,clientSecret:'line\nbreak'},{clientId:12,clientSecret:'FAKE_ONLY'}])assert.equal(validDesktopClient(JSON.stringify(value)),false);
  assert.equal(validDesktopClient(undefined),false);assert.equal(validDesktopClient('{not json'),false);
});
test('read and parse failures never echo a path, raw client data or exception details',()=>{
  const path='sensitive-file-name';const secret='FAKE_SECRET_MUST_NOT_APPEAR';
  const missing=checkFile(path,()=>{throw new Error(secret);});const malformed=checkFile(path,()=>'{'+secret);
  assert.equal(missing.ok,false);assert.equal(malformed.ok,false);assert.equal(missing.message,malformed.message);assert.ok(!missing.message.includes(path));assert.ok(!missing.message.includes(secret));
});
