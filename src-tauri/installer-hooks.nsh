; 사이를 지울 때 Windows 시작 프로그램 등록도 지운다.
; (업데이트 중에 지워지더라도 사이가 다시 켜질 때 스스로 다시 등록한다)
!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Sai"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Sai"
!macroend
