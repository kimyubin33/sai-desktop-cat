import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,existsSync,rmSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {createHash} from 'node:crypto';
import {peMachine,parseArgs,collectArtifacts,installedMatchesPortable} from './build-preview.mjs';
function pe(machine) {const bytes=Buffer.alloc(120);bytes.writeUInt16LE(0x5a4d);bytes.writeUInt32LE(64,0x3c);bytes.writeUInt32LE(0x4550,64);bytes.writeUInt16LE(machine,68);return bytes;}
function fixture(run) {const root=mkdtempSync(join(tmpdir(),'sai-preview-package-'));try{run(root);}finally{rmSync(root,{recursive:true,force:true});}}
function files(root,target,arch,machine) {const release=join(root,'target',target,'release');mkdirSync(join(release,'bundle','nsis'),{recursive:true});writeFileSync(join(release,'sai-preview.exe'),pe(machine));writeFileSync(join(release,'bundle','nsis',`사이 미리보기_0.9.14_${arch}-setup.exe`),pe(0x14c));return release;}
test('PE validation rejects truncated files, invalid offsets and signatures',()=>{
  assert.equal(peMachine(pe(0x8664)),0x8664);
  for(const bytes of [Buffer.alloc(8),Buffer.alloc(120),pe(0xaa64).subarray(0,70)])assert.throws(()=>peMachine(bytes));
  const overflow=pe(0x8664);overflow.writeUInt32LE(0xffffffff,0x3c);assert.throws(()=>peMachine(overflow));
});
test('CPU mismatch and missing installer cannot produce a preview distribution',()=>fixture(root=>{
  const target='x86_64-pc-windows-msvc';const release=files(root,target,'x64',0xaa64);
  assert.throws(()=>collectArtifacts(root,target,'0.9.14'),/CPU/);
  assert.equal(existsSync(join(root,'target','preview-dist')),false);
  writeFileSync(join(release,'sai-preview.exe'),pe(0x8664));rmSync(join(release,'bundle','nsis','사이 미리보기_0.9.14_x64-setup.exe'));
  assert.throws(()=>collectArtifacts(root,target,'0.9.14'));
  assert.equal(existsSync(join(root,'target','preview-dist')),false);
}));
test('x64 and ARM64 packages keep exact bytes, checksums and separate names without copying other artifacts',()=>fixture(root=>{
  for(const [target,arch,machine] of [['x86_64-pc-windows-msvc','x64',0x8664],['aarch64-pc-windows-msvc','arm64',0xaa64]]){
    const release=files(root,target,arch,machine);writeFileSync(join(release,'legacy-server.zip'),'must not ship');
    const result=collectArtifacts(root,target,'0.9.14');const sums=readFileSync(join(result.destination,'SHA256SUMS.txt'),'utf8');
    for(const name of result.files){const bytes=readFileSync(join(result.destination,name));assert.ok(sums.includes(`${createHash('sha256').update(bytes).digest('hex')}  ${name}\n`));}
    assert.deepEqual(readFileSync(join(result.destination,result.files[1])),pe(machine));
    assert.equal(existsSync(join(result.destination,'legacy-server.zip')),false);
    assert.equal(readFileSync(join(release,'legacy-server.zip'),'utf8'),'must not ship');
    const manifest=JSON.parse(readFileSync(join(result.destination,'preview-build.json'),'utf8'));assert.equal(manifest.identifier,'com.kimyubin33.sai.preview');assert.equal(manifest.teamAiConfigured,false);
  }
}));
test('only explicit Windows targets are accepted and cannot enter shell text or output paths',()=>{
  assert.deepEqual(parseArgs(['--target','x86_64-pc-windows-msvc','--ui-host-target','x86_64-pc-windows-msvc']),{target:'x86_64-pc-windows-msvc',uiHost:'x86_64-pc-windows-msvc'});
  for(const args of [['--target','..'],['--target','x64;echo invalid'],['--config','other'],['--target']])assert.throws(()=>parseArgs(args));
  assert.throws(()=>collectArtifacts('.', '../escape','0.9.14'));
  assert.throws(()=>collectArtifacts('.','x86_64-pc-windows-msvc','../escape'));
});
test('installed integrity accepts an exact binary or only the Tauri NSIS substitution without modifying inputs',()=>{
  const portable=Buffer.from('before__TAURI_BUNDLE_TYPE_VAR_UNKafter');
  const installed=Buffer.from('before__TAURI_BUNDLE_TYPE_VAR_NSSafter');
  const original=Buffer.from(portable);
  assert.equal(installedMatchesPortable(portable,portable),true);
  assert.equal(installedMatchesPortable(installed,portable),true);
  assert.deepEqual(portable,original);
});
test('a similar sized executable with any extra change, unknown marker or different length fails integrity',()=>{
  const portable=Buffer.from('before__TAURI_BUNDLE_TYPE_VAR_UNKafter');
  for(const installed of [Buffer.from('Before__TAURI_BUNDLE_TYPE_VAR_NSSafter'),Buffer.from('before__TAURI_BUNDLE_TYPE_VAR_MSIafter'),Buffer.from('before__TAURI_BUNDLE_TYPE_VAR_NSSafter!')])assert.equal(installedMatchesPortable(installed,portable),false);
  assert.equal(installedMatchesPortable(Buffer.from('different'),Buffer.from('same size')),false);
  assert.equal(installedMatchesPortable(null,portable),false);
});
test('only the first marker can differ even if more marker-shaped text exists in the executable',()=>{
  const portable=Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK__TAURI_BUNDLE_TYPE_VAR_UNK');
  assert.equal(installedMatchesPortable(Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS__TAURI_BUNDLE_TYPE_VAR_UNK'),portable),true);
  assert.equal(installedMatchesPortable(Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK__TAURI_BUNDLE_TYPE_VAR_NSS'),portable),false);
  assert.equal(installedMatchesPortable(Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS__TAURI_BUNDLE_TYPE_VAR_NSS'),portable),false);
});
