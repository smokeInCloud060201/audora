; Audora NSIS uninstaller hook
; Invokes `Audora.exe --uninstall-cleanup` (or `audora.exe`) and removes %APPDATA%\com.audora.desktop, %APPDATA%\com.audora.app, and %LOCALAPPDATA%\com.audora.desktop

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\Audora.exe" 0 _audora_try_lower
    nsExec::ExecToLog '"$INSTDIR\Audora.exe" --uninstall-cleanup'
    Goto _audora_cleanup_dirs
  _audora_try_lower:
  IfFileExists "$INSTDIR\audora.exe" 0 _audora_cleanup_dirs
    nsExec::ExecToLog '"$INSTDIR\audora.exe" --uninstall-cleanup'
  _audora_cleanup_dirs:
  RMDir /r "$APPDATA\com.audora.desktop"
  RMDir /r "$APPDATA\com.audora.app"
  RMDir /r "$LOCALAPPDATA\com.audora.desktop"
!macroend
