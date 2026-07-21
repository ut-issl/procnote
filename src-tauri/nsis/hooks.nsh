; Add $INSTDIR\bin to the current user's PATH and migrate the old CLI location.
!macro NSIS_HOOK_POSTINSTALL
  ; The pre-0.0.5 installer placed a second Tauri executable here. It must not
  ; remain ahead of the launcher because PATHEXT resolves .exe before .cmd.
  RMDir /r "$INSTDIR\cli"

  ; Use PowerShell's registry API instead of NSIS strings, which are limited to
  ; 1024 characters and can otherwise truncate a long user PATH.
  nsExec::ExecToLog /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\installer\update-user-path.ps1" install "$INSTDIR\bin" "$INSTDIR\cli"'
  Pop $0
  StrCmp $0 0 +2
    DetailPrint "Could not add the procnote launcher to user PATH (exit code $0)"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

; Remove both the current and legacy launcher directories from user PATH.
!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\installer\update-user-path.ps1" uninstall "$INSTDIR\bin" "$INSTDIR\cli"'
  Pop $0
  StrCmp $0 0 +2
    DetailPrint "Could not remove the procnote launcher from user PATH (exit code $0)"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend
