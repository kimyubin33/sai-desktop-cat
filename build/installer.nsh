; 사이 제거 시 Windows 시작 프로그램 등록도 지운다 (업데이트로 인한 제거일 때는 그대로 둔다)
!macro customUnInstall
  ${ifNot} ${isUpdated}
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Sai"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Sai"
  ${endIf}
!macroend
