"""Local operator menu included in the Windows desktop server bundle."""

import json
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime
from pathlib import Path


HEALTH = "http://127.0.0.1:8765/health"


def powershell():
    return str(Path(os.environ["SystemRoot"]) / "System32" / "WindowsPowerShell" / "v1.0" / "powershell.exe")


def running():
    # The host's proxy settings must not redirect a loopback health check.
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with client.open(HEALTH, timeout=1) as response:
            status = json.load(response)
        return (status.get("service"), status.get("protocol"), status.get("projectId")) == (
            "sai-team-server", 1, "saigajoa"
        )
    except (OSError, ValueError, AttributeError, urllib.error.URLError):
        return False


def launch(exe):
    if running():
        print("팀 서버가 이미 실행 중입니다.")
        return
    # A server may outlive the menu. Give the frozen child its own extraction directory.
    env = dict(os.environ, PYINSTALLER_RESET_ENVIRONMENT="1")
    process = subprocess.Popen([str(exe), "serve"], env=env,
                               creationflags=subprocess.CREATE_NEW_CONSOLE)
    for _ in range(45):
        if running():
            print("팀 서버가 실행 중입니다. 종료하려면 서버 창에서 Ctrl+C를 누르세요.")
            return
        if process.poll() is not None:
            break
        time.sleep(0.5)
    print("팀 서버를 시작하지 못했습니다. 서버 창의 메시지와 8765번 포트를 확인하세요.")


def startup(exe):
    # Ask Windows for the user's redirected or localized Startup folder.
    script = r"""
$ErrorActionPreference = 'Stop'
$sh = New-Object -ComObject WScript.Shell
$link = Join-Path ([Environment]::GetFolderPath('Startup')) 'Sai Desktop Team Server.lnk'
if ($env:SAI_STARTUP_ACTION -eq 'remove') {
    if (Test-Path -LiteralPath $link) { Remove-Item -LiteralPath $link }
    return
}
$shortcut = $sh.CreateShortcut($link)
$shortcut.TargetPath = $env:SAI_STARTUP_EXE
$shortcut.Arguments = 'serve'
$shortcut.WorkingDirectory = Split-Path -Parent $env:SAI_STARTUP_EXE
$shortcut.WindowStyle = 7
$shortcut.Save()
"""
    action = input("1. 로그인 시 자동 시작 등록    2. 자동 시작 해제    Enter. 돌아가기\n> ").strip()
    if action not in {"1", "2"}:
        return
    env = dict(os.environ, SAI_STARTUP_ACTION="install" if action == "1" else "remove",
               SAI_STARTUP_EXE=str(exe))
    subprocess.run([powershell(), "-NoProfile", "-NonInteractive", "-Command", script],
                   env=env, check=True)
    if action == "1":
        print(f"다음 Windows 로그인부터 서버가 시작됩니다. 실행 파일 위치: {exe}")
        launch(exe)
    else:
        print("로그인 시 자동 시작을 해제했습니다. 실행 중인 서버는 서버 창에서 종료하세요.")


def offer_download(label, url):
    print(f"{label} 설치 안내: {url}")
    if input("공식 설치 페이지를 열려면 1, 돌아가려면 Enter를 누르세요.\n> ").strip() == "1":
        os.startfile(url)


def private_address():
    # Only elevate the installed binary under Program Files.
    tailscale = Path(os.environ["ProgramFiles"]) / "Tailscale" / "tailscale.exe"
    if not tailscale.is_file():
        offer_download("Tailscale", "https://tailscale.com/download/windows")
        return
    if not running():
        print("먼저 1번으로 사이 서버를 실행하세요.")
        return
    try:
        result = subprocess.check_output([tailscale, "serve", "status", "--json"], timeout=15)
        config = json.loads(result) or {}
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, ValueError):
        print("Tailscale의 상태를 읽지 못했습니다. 작업 표시줄에서 Tailscale에 로그인하고 다시 시도하세요.")
        return
    if any(config.get("AllowFunnel", {}).values()):
        print("이 PC에 외부 공개(Funnel) 설정이 있습니다. 관리자와 기존 설정을 확인한 뒤 팀 전용 주소를 연결하세요.")
        return
    for name, site in config.get("Web", {}).items():
        proxy = site.get("Handlers", {}).get("/", {}).get("Proxy", "").rstrip("/")
        if name.endswith(":443") and proxy in ("http://127.0.0.1:8765", "http://localhost:8765"):
            print("팀원용 주소: https://" + name[:-4])
            return
    if any(config.values()):
        print("이 PC에서 다른 Tailscale 서비스가 실행 중입니다. 기존 주소를 보존했습니다. 관리자와 사용할 경로를 확인하세요.")
        subprocess.run([tailscale, "serve", "status"], check=True)
        return
    print("이 데스크톱에 접근 권한이 있는 Tailscale 팀원에게 사이 서버를 HTTPS로 연결합니다.")
    if input("설정하려면 1, 돌아가려면 Enter를 누르세요.\n> ").strip() != "1":
        return
    # The elevated process only receives fixed Serve settings and the executable path.
    script = """
$ErrorActionPreference = 'Stop'
$job = Start-Process -FilePath $env:SAI_TAILSCALE_EXE -Verb RunAs -PassThru -Wait -ArgumentList 'serve','--bg','http://127.0.0.1:8765'
exit $job.ExitCode
"""
    subprocess.run([powershell(), "-NoProfile", "-NonInteractive", "-Command", script],
                   env=dict(os.environ, SAI_TAILSCALE_EXE=str(tailscale)), check=True)
    subprocess.run([tailscale, "serve", "status"], check=True)
    print("위의 https:// 주소를 Tailscale 연결을 마친 팀원에게 전달하세요.")


def migration_login():
    env = os.environ.copy()
    gcloud = shutil.which("gcloud.cmd")
    if not gcloud:
        for root in ("LOCALAPPDATA", "ProgramFiles(x86)", "ProgramFiles"):
            folder = Path(env.get(root, "")) / "Google" / "Cloud SDK" / "google-cloud-sdk" / "bin"
            if (folder / "gcloud.cmd").is_file():
                gcloud = str(folder / "gcloud.cmd")
                env["PATH"] = str(folder) + os.pathsep + env.get("PATH", "")
                break
    if not gcloud:
        offer_download("Google Cloud CLI", "https://cloud.google.com/sdk/docs/install#windows")
        return None
    active = subprocess.run([gcloud, "auth", "list", "--filter=status:ACTIVE", "--format=value(account)"],
                            capture_output=True, env=env)
    if active.returncode or not active.stdout.strip():
        if input("Google Cloud 계정 로그인을 시작하려면 1, 돌아가려면 Enter를 누르세요.\n> ").strip() != "1":
            return None
        subprocess.run([gcloud, "auth", "login"], env=env, check=True)
    return env


def migrate(exe):
    print("기존 방을 복사합니다. 이전부터 팀원 모두가 주소를 바꿀 때까지 기존 방의 입력을 잠시 멈추세요.")
    print("복사본에는 승인한 개인 조언·팀 공유 연동 정보·파일 링크가 포함됩니다. 본인 Windows 계정에 보관하세요.")
    print("이 컴퓨터에 Google Cloud CLI가 설치되어 있고 프로젝트를 읽을 수 있는 계정으로 로그인되어야 합니다.")
    codes = input("복사할 팀 참여 코드 또는 팀 ID (여러 팀은 공백으로 구분, Enter로 취소)\n> ").split()
    if codes:
        env = migration_login()
        if env is None:
            return
        args = [str(exe), "migrate"]
        for code in codes:
            args.extend(["--team", code])
        subprocess.run(args, env=env, check=True)


def backup(exe):
    folder = Path(os.environ["LOCALAPPDATA"]) / "SaiTeamServer" / "backups"
    proposed = folder / datetime.now().strftime("team-%Y%m%d-%H%M%S.sqlite3")
    print(f"기본 백업 파일: {proposed}")
    entered = input("Enter로 기본 위치에 저장합니다. 다른 경로를 입력하거나 0으로 취소하세요.\n> ").strip()
    if entered == "0":
        return
    if not entered:
        folder.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(exe), "backup", "--output", entered.strip('"') if entered else str(proposed)], check=True)


def run_setup():
    if os.name != "nt" or not getattr(sys, "frozen", False):
        print("이 메뉴는 Windows 배포 파일의 Start.cmd에서 실행하세요.", file=sys.stderr)
        return 1
    exe = Path(sys.executable).resolve()
    print("사이 데스크톱 팀 서버")
    print("로컬 주소: http://127.0.0.1:8765")
    print(f"팀 데이터: {Path(os.environ['LOCALAPPDATA']) / 'SaiTeamServer' / 'team.sqlite3'}")
    while True:
        print("\n1. 서버 실행               2. 서버 상태 확인\n"
              "3. 기존 Firebase 팀 복사   4. 팀 데이터 백업\n"
              "5. 로그인 시 자동 시작 설정  6. 새 팀 개설자 등록\n"
              "7. 설치 안내 열기          8. 팀원용 HTTPS 주소 설정 (Tailscale)\n0. 메뉴 닫기")
        try:
            choice = input("> ").strip()
            if choice == "0":
                return 0
            if choice == "1":
                launch(exe)
            elif choice == "2":
                print("사이 팀 서버에 연결됐습니다." if running() else "팀 서버 응답이 없습니다. 1번으로 실행하세요.")
            elif choice == "3":
                migrate(exe)
            elif choice == "4":
                backup(exe)
            elif choice == "5":
                startup(exe)
            elif choice == "6":
                uid = input("새 팀을 개설할 사용자의 사이 팀 계정 UID (Enter로 취소)\n> ").strip()
                if uid:
                    subprocess.run([str(exe), "allow-creator", "--uid", uid], check=True)
            elif choice == "7":
                os.startfile(str(exe.with_name("Server-Guide.txt")))
            elif choice == "8":
                private_address()
        except (EOFError, KeyboardInterrupt):
            return 0
        except subprocess.CalledProcessError as error:
            print(f"작업을 완료하지 못했습니다 (종료 코드 {error.returncode}). 위의 안내를 확인하세요.")
        except OSError as error:
            print(f"작업을 완료하지 못했습니다: {error}")
