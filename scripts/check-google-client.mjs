// Validate developer-owned desktop OAuth packaging without logging client data.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const message='Google 데스크톱 연결 정보가 없거나 형식이 올바르지 않습니다. 개발 측 GOOGLE_CLIENT_JSON을 확인해 주세요. 사용자에게 JSON 설정을 요구하는 설치 파일은 만들지 않습니다.';
export function validDesktopClient(raw) {
  try {
    const parsed=JSON.parse(raw); const client=parsed?.installed??parsed;
    if(!client||typeof client!=='object'||Array.isArray(client))return false;
    const id=client.clientId??client.client_id;const secret=client.clientSecret??client.client_secret;
    return typeof id==='string'&&id.length<=255&&/^[a-zA-Z0-9._-]+\.apps\.googleusercontent\.com$/.test(id)
      &&typeof secret==='string'&&secret.length>0&&secret.length<=4096&&/^[\x21-\x7e]+$/.test(secret);
  } catch { return false; }
}
export function checkFile(path,read=readFileSync) {
  try { if(validDesktopClient(read(path,'utf8')))return {ok:true,message:'Google 데스크톱 연결 정보 형식을 확인했습니다.'}; } catch {}
  return {ok:false,message};
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const path=resolve('config/google-client.json');
  if(process.argv[2]==='--write-env') {
    const raw=process.env.GOOGLE_CLIENT_JSON;
    if(!validDesktopClient(raw)){console.error(message);process.exitCode=1;}
    else {try{mkdirSync(resolve('config'),{recursive:true});writeFileSync(path,raw,{encoding:'utf8'});}catch{console.error('Google 연결 정보를 빌드 폴더에 저장하지 못했습니다.');process.exitCode=1;}}
  }
  if(!process.exitCode){const result=checkFile(path);(result.ok?console.log:console.error)(result.message);if(!result.ok)process.exitCode=1;}
}
