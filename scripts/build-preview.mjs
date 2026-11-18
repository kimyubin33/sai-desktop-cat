// One entry point for a separately installed Windows preview. No credentials are printed.
import {readFileSync,writeFileSync,mkdirSync,copyFileSync,existsSync} from 'node:fs';
import {resolve,join,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {checkFile} from './check-google-client.mjs';

export const targets=Object.freeze({
  'x86_64-pc-windows-msvc':{arch:'x64',machine:0x8664},
  'aarch64-pc-windows-msvc':{arch:'arm64',machine:0xaa64},
});
const product='사이 미리보기';
const identifier='com.kimyubin33.sai.preview';
export function peMachine(bytes) {
  if(bytes.length<64||bytes.readUInt16LE(0)!==0x5a4d)throw new Error('실행 파일 형식을 확인하지 못했습니다.');
  const offset=bytes.readUInt32LE(0x3c);
  if(offset<64||offset>bytes.length-24||bytes.readUInt32LE(offset)!==0x4550)throw new Error('실행 파일 헤더를 확인하지 못했습니다.');
  return bytes.readUInt16LE(offset+4);
}
export function installedMatchesPortable(installed,portable) {
  if(!Buffer.isBuffer(installed)||!Buffer.isBuffer(portable)||installed.length!==portable.length)return false;
  if(installed.equals(portable))return true;
  // Tauri patches the first UNK marker for NSIS and restores the original for portable output.
  // Compare all bytes against that single documented substitution, without modifying either file.
  const original=Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
  const nsis=Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS');
  const offset=portable.indexOf(original);
  if(offset<0)return false;
  const expected=Buffer.from(portable);
  nsis.copy(expected,offset);
  return installed.equals(expected);
}
export function parseArgs(args,arch=process.arch) {
  const result={target:arch==='arm64'?'aarch64-pc-windows-msvc':'x86_64-pc-windows-msvc',uiHost:null};
  for(let n=0;n<args.length;n++) {
    const key=args[n];
    if(!['--target','--ui-host-target'].includes(key)||!Object.hasOwn(targets,args[n+1]))throw new Error('지원하는 Windows 빌드 대상을 지정해 주세요.');
    result[key==='--target'?'target':'uiHost']=args[++n];
  }
  return result;
}
export function collectArtifacts(root,target,version,teamAiConfigured=false) {
  const spec=targets[target];
  if(!spec||!/^\d+\.\d+\.\d+$/.test(version))throw new Error('검증판의 대상 또는 버전이 올바르지 않습니다.');
  const release=join(root,'target',target,'release');
  const binary=join(release,'sai-preview.exe');
  const installer=join(release,'bundle','nsis',`${product}_${version}_${spec.arch}-setup.exe`);
  // Validate every input before making publishable-looking copies. NSIS itself has an x86 launcher.
  const binaryBytes=readFileSync(binary);
  if(peMachine(binaryBytes)!==spec.machine)throw new Error('검증 앱의 CPU가 빌드 대상과 일치하지 않습니다.');
  const installerBytes=readFileSync(installer);
  if(![0x14c,0x8664,0xaa64].includes(peMachine(installerBytes)))throw new Error('검증 설치 파일을 확인하지 못했습니다.');
  const destination=join(root,'target','preview-dist',`win-${spec.arch}`);
  mkdirSync(destination,{recursive:true});
  const suffix=spec.arch==='arm64'?'-arm64':'';
  const files=[
    [`Sai-Preview-Setup-${version}${suffix}.exe`,installer,installerBytes],
    [`Sai-Preview-Portable-${version}${suffix}.exe`,binary,binaryBytes],
  ];
  for(const [name,source] of files)copyFileSync(source,join(destination,name));
  writeFileSync(join(destination,'SHA256SUMS.txt'),files.map(([name,,bytes])=>`${createHash('sha256').update(bytes).digest('hex')}  ${name}`).join('\n')+'\n');
  writeFileSync(join(destination,'preview-build.json'),JSON.stringify({version,target,product,identifier,binary:'sai-preview.exe',teamAiConfigured,createdAt:new Date().toISOString()},null,2)+'\n');
  writeFileSync(join(destination,'먼저 읽어주세요.txt'),
    `사이 미리보기 ${version} · Windows ${spec.arch}\n\n`+
    '1. Setup 파일을 실행해 설치하거나 Portable 파일을 바로 실행하세요.\n'+
    '2. Google →을 누르고 브라우저에서 본인 계정으로 로그인하세요.\n'+
    '3. 같은 계정의 이름·설정·참여 팀을 복원하고, 파일 보기에서 자료를 올리고 받을 수 있습니다.\n'+
    '4. 내 일정은 사용할 때 Calendar 권한을 한 번 따로 연결합니다. 확인한 일정만 등록·수정·삭제합니다.\n\n'+
    '기존 사이와 별도로 설치되며 기존 PC 자료를 자동으로 가져오거나 지우지 않습니다.\n'+
    '미리보기는 시작 프로그램·자동 업데이트에 등록하지 않습니다. 창을 닫으면 종료합니다.\n'+
    '이 검증판은 별도 코드 서명을 하지 않았습니다. 설치와 Google 동의는 사용자가 직접 진행합니다.\n'+
    (teamAiConfigured?'팀 Claude 연결 정보가 포함됐습니다. 실제 서비스 사용 가능 여부는 앱에서 확인합니다.\n':'팀 Claude 서비스는 준비 중입니다. 파일·협업·Calendar 검증은 진행할 수 있습니다.\n')+
    'Google 계정·두 PC·설치·권한 철회 전체 시험을 완료한 정식 배포판은 아닙니다.\n');
  return {destination,files:files.map(([name])=>name)};
}

export function build(args) {
  if(process.platform!=='win32')throw new Error('Windows에서 검증 설치 파일을 만들어 주세요.');
  const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
  const {target,uiHost}=parseArgs(args);
  const client=checkFile(join(root,'config','google-client.json'));
  if(!client.ok)throw new Error(client.message);
  const cli=join(root,'node_modules','@tauri-apps','cli','tauri.js');
  if(!existsSync(cli))throw new Error('먼저 npm ci로 저장소의 빌드 도구를 설치해 주세요.');
  const overlay=JSON.parse(readFileSync(join(root,'src-tauri','tauri.preview.conf.json'),'utf8'));
  if(overlay.identifier!==identifier||overlay.productName!==product||overlay.mainBinaryName!=='sai-preview'||overlay.bundle?.windows?.nsis?.installerHooks!==null)throw new Error('검증판의 독립 설치 설정을 확인해 주세요.');
  // Override only the host running xtask on a cross-compiling development PC.
  if(uiHost)overlay.build={beforeBuildCommand:`cargo run --locked -p xtask --target ${uiHost} -- ui`};
  const configDir=join(root,'target','preview-build',targets[target].arch);
  mkdirSync(configDir,{recursive:true});
  const config=join(configDir,'config.json');
  writeFileSync(config,JSON.stringify(overlay));
  console.log(`Windows ${targets[target].arch} 독립 미리보기를 만듭니다.`);
  const result=spawnSync(process.execPath,[cli,'build','--ci','--target',target,'--config',config,'--features','preview','--bundles','nsis','--','--locked'],{
    cwd:root,stdio:'inherit',env:{...process.env,CARGO_TARGET_DIR:join(root,'target')},windowsHide:true,
  });
  if(result.error||result.status!==0)throw new Error('검증 설치 파일 빌드가 완료되지 않았습니다. 위 빌드 오류를 확인해 주세요.');
  const cargo=readFileSync(join(root,'Cargo.toml'),'utf8');
  const version=cargo.match(/^version\s*=\s*"(\d+\.\d+\.\d+)"/m)?.[1];
  const artifact=collectArtifacts(root,target,version,Boolean(process.env.SAI_TEAM_AI_ENDPOINT));
  console.log(`검증 파일과 체크섬을 만들었습니다: ${artifact.destination}`);
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  try{build(process.argv.slice(2));}catch(error){
    // Filesystem/runtime exception details can contain local paths. Keep them out of distributed logs.
    console.error(error?.code?'검증 파일을 읽거나 저장하지 못했습니다. 빌드 결과를 확인해 주세요.':error.message);
    process.exitCode=1;
  }
}
