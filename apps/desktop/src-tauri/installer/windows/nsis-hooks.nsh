; Audora NSIS uninstaller hook
; Invokes `audora.exe --uninstall-cleanup` and removes %APPDATA%\com.audora.desktop and %LOCALAPPDATA%\com.audora.desktop

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\audora.exe" 0 +2
    nsExec::ExecToLog '"$INSTDIR\audora.exe" --uninstall-cleanup'
  RMDir /r "$APPDATA\com.audora.desktop"
  RMDir /r "$APPDATA\com.audora.app"
  RMDir /r "$LOCALAPPDATA\com.audora.desktop"
!macroend
