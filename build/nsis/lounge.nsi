; Lounge's Windows installer (Lounge-Setup-<version>.exe), built by CI with makensis:
;   makensis /DVERSION=3.0.0 /DVERSION_4=3.0.0.0 /DSRC=dist\win-unpacked /DOUT=dist\Lounge-Setup-3.0.0.exe build\nsis\lounge.nsi
;
; Behaves like the electron-builder installer Lounge 2.x shipped, which the updater relies on
; (lounge-core/src/updater.rs):
; - per-user, into %LOCALAPPDATA%\Programs\Lounge, so it installs over 2.x in place and never needs UAC;
; - "Uninstall Lounge.exe" next to Lounge.exe — that's how the updater recognises an installed copy;
; - /S installs silently (the updater runs `/S --updated --force-run`); --force-run starts Lounge after.
; User data (%APPDATA%\com.foyer.launcher and the 2.x folders) is never touched.

Unicode true
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
; Four-part version for the file properties (pre-release suffixes dropped): 3.0.0-alpha -> 3.0.0.0.
!ifndef VERSION_4
  !define VERSION_4 "0.0.0.0"
!endif
!ifndef SRC
  !define SRC "..\..\dist\win-unpacked"
!endif
!ifndef OUT
  !define OUT "..\..\dist\Lounge-Setup-${VERSION}.exe"
!endif

!define NAME "Lounge"
!define EXE "Lounge.exe"
!define UNINSTALLER "Uninstall Lounge.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"

Name "${NAME}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${NAME}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
Icon "..\..\app\icons\icon.ico"
UninstallIcon "..\..\app\icons\icon.ico"
BrandingText "${NAME} ${VERSION}"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define MUI_ICON "..\..\app\icons\icon.ico"
!define MUI_UNICON "..\..\app\icons\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${NAME}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "French"

VIProductVersion "${VERSION_4}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${NAME}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${NAME} installer"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"

; Wait for a running Lounge to exit (the updater quits it first; give it a moment), then make sure.
!macro CloseLounge
  StrCpy $0 0
  ${Do}
    nsExec::Exec 'cmd /c tasklist /FI "IMAGENAME eq ${EXE}" /NH | find /I "${EXE}"'
    Pop $1
    ${If} $1 != 0
      ${Break}
    ${EndIf}
    IntOp $0 $0 + 1
    ${If} $0 > 20
      nsExec::Exec 'taskkill /F /IM "${EXE}"'
      Sleep 500
      ${Break}
    ${EndIf}
    Sleep 250
  ${Loop}
!macroend

Section "Install"
  !insertmacro CloseLounge
  SetOutPath "$INSTDIR"
  File /r "${SRC}\*"
  WriteUninstaller "$INSTDIR\${UNINSTALLER}"

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${EXE}"
  ${IfNot} ${FileExists} "$DESKTOP\${NAME}.lnk"
    ${IfNot} ${Silent}
      CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${EXE}"
    ${EndIf}
  ${EndIf}

  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0

  ; The updater asks for Lounge to be started again once the silent install is done.
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "--force-run" $R1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\${EXE}"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  !insertmacro CloseLounge
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${UNINSTALLER}"
  RMDir /r "$INSTDIR"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$DESKTOP\${NAME}.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  ; The launch-at-login entry, if Settings turned it on.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${NAME}"
SectionEnd
