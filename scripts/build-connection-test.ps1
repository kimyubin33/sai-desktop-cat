[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$buildTarget = 'x86_64-pc-windows-msvc'
$packageDirectory = Join-Path $projectRoot 'target\connection-test-win-x64'
$archivePath = Join-Path $projectRoot 'target\Sai-Connection-Test-win-x64.zip'

Push-Location -LiteralPath $projectRoot
try {
    & cargo run --locked -p xtask --target $buildTarget -- ui
    if ($LASTEXITCODE -ne 0) { throw '진단 화면 빌드에 실패했습니다.' }
    & cargo build --locked -p sai --bin sai-connection-test --features 'connection-test,tauri/custom-protocol' --target $buildTarget --profile connection-test
    if ($LASTEXITCODE -ne 0) { throw '진단 실행 파일 빌드에 실패했습니다.' }

    New-Item -ItemType Directory -Path $packageDirectory -Force | Out-Null
    $builtExecutable = Join-Path $projectRoot "target\$buildTarget\connection-test\sai-connection-test.exe"
    $packagedExecutable = Join-Path $packageDirectory 'Sai-Connection-Test.exe'
    Copy-Item -LiteralPath $builtExecutable -Destination $packagedExecutable -Force
    $guide = Get-Content -LiteralPath (Join-Path $projectRoot 'docs\mesh-connection-experiment.md') -Raw -Encoding UTF8
    $guide = $guide.Replace('[자동 호스트와 PC 간 동기화 설계](automatic-host-design.md)', '저장소의 docs/automatic-host-design.md')
    Set-Content -LiteralPath (Join-Path $packageDirectory '실행안내.md') -Value $guide -Encoding UTF8

    $quickStart = @'
사이 PC 연결 진단 — 실험 빌드

1. ZIP의 압축을 모두 풀고 Sai-Connection-Test.exe를 실행하세요.
2. 같은 LAN의 두 Windows PC에서 각각 실행하세요.
3. 실제 Wi-Fi 또는 유선 LAN 주소를 고르고 동의 후 실험을 시작하세요.
4. 공개 연결 정보를 서로 전달해 양쪽에 등록하고 인증서 지문을 대조하세요.
5. 양쪽에서 연결 확인을 누르세요. 성공 시각을 확인하세요.
6. 한쪽에서 중지한 뒤 다른 PC에서 다시 확인하면 실패해야 합니다.
7. 창을 닫으면 진단 앱과 연결 실험이 종료됩니다.

기존 사이의 팀 기록, 로그인, 자동 시작 설정을 사용하지 않습니다.
실험을 직접 시작하기 전에는 LAN 발견과 수신 포트를 열지 않습니다.
이번 앱은 팀 톡·파일을 전송하지 않고 연결 확인 신호만 주고받습니다.
사설 IPv4 LAN과 Windows의 WebView2 실행 환경이 필요합니다.
Windows 통신 허용과 네트워크의 PC 간 통신 허용이 필요할 수 있습니다.
앱은 방화벽을 변경하지 않으며 원격 명령 실행 기능이 없습니다.
이 파일은 서명되지 않은 x64 진단 빌드입니다. ARM64 네이티브 빌드가 아닙니다.

자세한 절차: 실행안내.md
실제 두 PC의 결과를 적을 항목: 검증기록.txt
'@
    Set-Content -LiteralPath (Join-Path $packageDirectory '먼저읽기.txt') -Value $quickStart -Encoding UTF8
    $checklist = @'
PC 연결 진단 결과

날짜:
PC A의 Windows 버전과 CPU 종류:
PC B의 Windows 버전과 CPU 종류:
연결: 같은 Wi-Fi / 같은 유선 LAN / 기타

[ ] 두 PC에서 창이 열리고 LAN 주소가 보인다.
[ ] 시작 전에는 실험이 꺼져 있다.
[ ] 시작 후 상대 실험 장치가 발견된다.
[ ] 양쪽에 공개 정보를 등록하고 상대 인증서 지문을 대조했다.
[ ] A에서 B로 연결 확인에 성공했다.
[ ] B에서 A로 연결 확인에 성공했다.
[ ] A의 실험 중지 후 B의 재확인이 실패했다.
[ ] 재시작 후 예전 정보가 새 장치 인증으로 인정되지 않았다.
[ ] 새 정보를 교환한 뒤 다시 양방향 연결에 성공했다.
[ ] 창 종료 후 연결 실험도 끝났다.
[ ] 절전·복귀 후 상태를 다시 확인했다.
[ ] 네트워크 변경 후 실패와 재시작 안내를 확인했다.

실패한 단계와 화면에 나온 문구:

개인키·로그인 토큰·개인 대화는 기록하거나 전달하지 마세요.
'@
    Set-Content -LiteralPath (Join-Path $packageDirectory '검증기록.txt') -Value $checklist -Encoding UTF8
    $hash = (Get-FileHash -LiteralPath $packagedExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $packageDirectory 'SHA256SUMS.txt') -Value "$hash  Sai-Connection-Test.exe" -Encoding ASCII
    Compress-Archive -LiteralPath @(
        $packagedExecutable,
        (Join-Path $packageDirectory '실행안내.md'),
        (Join-Path $packageDirectory '먼저읽기.txt'),
        (Join-Path $packageDirectory '검증기록.txt'),
        (Join-Path $packageDirectory 'SHA256SUMS.txt')
    ) -DestinationPath $archivePath -Force
    Write-Output "진단 ZIP: $archivePath"
    Write-Output "실행 파일: $packagedExecutable"
    Write-Output "SHA256: $hash"
}
finally {
    Pop-Location
}
