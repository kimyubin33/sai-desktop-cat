$ErrorActionPreference = 'Stop'
$version = $env:SAI_BUILD_VERSION
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Set SAI_BUILD_VERSION to the release version.' }
$work = Join-Path (Resolve-Path '.').Path 'target/team-server-package'
$bundle = Join-Path $work 'bundle'
New-Item -ItemType Directory -Force $work, $bundle | Out-Null
python -m PyInstaller --noconfirm --clean --onefile --python-option "X utf8" --name Sai-Team-Server --paths . --collect-data certifi --distpath $bundle --specpath $work --workpath "$work/build" server/start_server.py
if ($LASTEXITCODE) { exit $LASTEXITCODE }

$exe = Join-Path $bundle 'Sai-Team-Server.exe'
Get-Content server/windows/Start.cmd | Set-Content (Join-Path $bundle 'Start.cmd') -Encoding ascii
Get-Content docs/desktop-server.md -Raw -Encoding UTF8 | Set-Content (Join-Path $bundle 'Server-Guide.txt') -Encoding utf8BOM
$db = Join-Path $work ("smoke-" + [guid]::NewGuid() + '.sqlite3')
$server = Start-Process -FilePath $exe -ArgumentList 'serve','--port','18765','--db',('"' + $db + '"') -PassThru
try {
    $health = $null
    for ($i = 0; $i -lt 45; $i++) {
        if ($server.HasExited) { throw 'Packaged server exited before its health check.' }
        try { $health = Invoke-RestMethod -Method Get -Uri 'http://127.0.0.1:18765/health' -TimeoutSec 2 } catch { }
        if ($health) { break }
        Start-Sleep -Seconds 1
    }
    if ($health.service -ne 'sai-team-server' -or $health.projectId -ne 'saigajoa' -or $health.protocol -ne 1) {
        throw 'Packaged server health contract failed.'
    }
    & $exe migrate --help
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    "0" | & $exe setup
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
} finally {
    if (!$server.HasExited) {
        $server.Kill($true)
        $server.WaitForExit()
    }
}
New-Item -ItemType Directory -Force dist | Out-Null
$archive = "dist/Sai-Team-Server-$version-win-x64.zip"
$files = @( $exe, (Join-Path $bundle 'Start.cmd'), (Join-Path $bundle 'Server-Guide.txt') )
Compress-Archive -LiteralPath $files -DestinationPath $archive -Force
Write-Host "Created $archive"
