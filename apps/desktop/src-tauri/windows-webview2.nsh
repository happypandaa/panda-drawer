; The stock WebView2 section aborts installation if the bootstrapper fails.
; This hook lets the app finish installing and retains its startup repair dialog.
!define PANDA_WEBSITE "https://panda.zhimibuhui.com"
!define PANDA_WEBVIEW2_DOWNLOAD "https://go.microsoft.com/fwlink/?linkid=2124701"

; Tauri defines WEBVIEW2APPGUID before inserting NSIS_HOOK_PREINSTALL.
; Return 1 in $0 if either the machine or current user has a positive version.
!macro PandaDetectWebView2
  ReadRegStr $0 HKLM32 "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${VersionCompare} "$0" "0.0.0.0" $0
  ${If} $0 != "1"
    ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
    ${VersionCompare} "$0" "0.0.0.0" $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  Push $0
  Push $1
  Push $2
  Push $3
  ${If} $UpdateMode = 1
    Goto panda_webview2_done
  ${EndIf}
  !insertmacro PandaDetectWebView2
  ${If} $0 == "1"
    Goto panda_webview2_done
  ${EndIf}

  InitPluginsDir
  ; Tauri's installer is a 32-bit process on x64 Windows.
  StrCpy $2 "$SYSDIR\curl.exe"
  ${If} ${FileExists} "$WINDIR\Sysnative\curl.exe"
    StrCpy $2 "$WINDIR\Sysnative\curl.exe"
  ${EndIf}
  DetailPrint "Downloading Microsoft Edge WebView2 Runtime..."
  nsExec::ExecToLog '"$2" --disable --fail --silent --show-error --location --proto "=https" --proto-redir "=https" --connect-timeout 15 --max-time 120 --max-filesize 8388608 --output "$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"'
  Pop $0
  StrCpy $1 "download: $0"
  ${If} $0 == "0"
    DetailPrint "Installing Microsoft Edge WebView2 Runtime..."
    ; nsExec measures output inactivity; output resets this 600-second timeout.
    nsExec::ExecToLog /TIMEOUT=600000 '"$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install'
    Pop $0
    StrCpy $1 "setup: $0"
  ${EndIf}
  Delete "$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe"
  !insertmacro PandaDetectWebView2
  ${If} $0 == "1"
    Goto panda_webview2_done
  ${EndIf}

  DetailPrint "Microsoft Edge WebView2 Runtime is unavailable ($1). ${PANDA_WEBVIEW2_DOWNLOAD}"
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
    ; The installer has one UI language by default. Use the Windows UI language.
    System::Call 'kernel32::GetUserDefaultUILanguage() i.r0'
    IntOp $0 $0 & 0x3ff
    ${If} $0 = 4
      StrCpy $0 "WebView2 自动安装失败。熊猫小抽屉会继续安装。现在查看安装教程吗？如果教程暂时打不开，将直接下载微软官方 64 位离线安装包（体积可达数百 MB）。下载后运行安装包，装好后重新打开熊猫小抽屉。"
      StrCpy $3 "${PANDA_WEBSITE}/wiki/webview2/"
    ${Else}
      StrCpy $0 "Automatic WebView2 setup failed. Panda Drawer will finish installing. Open the installation guide now? If the guide is unavailable, this will download Microsoft's official 64-bit offline installer instead (which can be several hundred MB). Run the downloaded installer, then reopen Panda Drawer."
      StrCpy $3 "${PANDA_WEBSITE}/en/wiki/webview2/"
    ${EndIf}
    MessageBox MB_ICONEXCLAMATION|MB_YESNO|MB_DEFBUTTON2 "$0$\r$\n$\r$\n${PANDA_WEBVIEW2_DOWNLOAD}$\r$\n($1)" /SD IDNO IDNO panda_webview2_done
    ; Probe only the requested guide; the official download also works without our site.
    nsExec::Exec '"$2" --disable --head --fail --silent --location --proto "=https" --proto-redir "=https" --connect-timeout 2 --max-time 4 --output NUL "$3"'
    Pop $0
    ${If} $0 != "0"
      StrCpy $3 "${PANDA_WEBVIEW2_DOWNLOAD}"
    ${EndIf}
    ClearErrors
    ExecShell "open" "$3"
    ${If} ${Errors}
      MessageBox MB_ICONINFORMATION|MB_OK "${PANDA_WEBVIEW2_DOWNLOAD}" /SD IDOK
    ${EndIf}
  ${EndIf}

  panda_webview2_done:
  ClearErrors
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend
