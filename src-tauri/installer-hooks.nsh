!include "FileFunc.nsh"

; 예전(Electron) 사이가 자동 업데이트로 이 설치 파일을 실행하면(--updated) 설치가 끝난 뒤 새 사이를 켠다
!macro NSIS_HOOK_POSTINSTALL
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "--updated" $R1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\sai.exe"'
  ${EndIf}
!macroend

; 사이를 지울 때 Windows 시작 프로그램 등록도 지운다.
; (업데이트 중에 지워지더라도 사이가 다시 켜질 때 스스로 다시 등록한다)
!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Sai"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Sai"
!macroend
