; Chippy's Windows installer (Inno Setup 6).
;
;   iscc /DAppVersion=1.2.3 /DFileVersion=1.2.3 /DSourceDir=path\to\files packaging\windows\chippy.iss
;
; AppVersion is shown to people (it may be 0.1.0-abc1234 for a test build);
; FileVersion is the numeric one Windows keeps in the file's properties.
; SourceDir holds chippy.exe, README.md and LICENSE; the installer is
; written to OutputDir (default: dist beside the repository root).
;
; It installs for the current user by default (no administrator prompt,
; into %LOCALAPPDATA%\Programs\Chippy), or for everyone if chosen. The
; uninstaller asks whether to delete the settings too (not when run silently,
; as by winget: then they are kept).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef FileVersion
  #define FileVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\windows"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

#define AppName "Chippy"
#define AppExe "chippy.exe"
#define ProgId "Chippy.Music"

[Setup]
; Never change the AppId: it is how a new version finds the old one to
; replace, and how Windows lists it under Installed apps.
AppId={{6F1C2B9E-4D7A-4E1B-9C3F-2A8D5E7B1C40}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Chippy
AppPublisherURL=https://github.com/singularity098/Chippy
AppSupportURL=https://github.com/singularity098/Chippy/issues
AppUpdatesURL=https://github.com/singularity098/Chippy/releases
VersionInfoVersion={#FileVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
; Per user unless they choose otherwise in the first dialog.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=chippy.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=Chippy-{#AppVersion}-windows-x86_64
Compression=lzma2/max
SolidCompression=yes
; Tell Explorer when the Open with entries change.
ChangesAssociations=yes
; A running Chippy is closed (and offered a restart) instead of blocking
; the update.
CloseApplications=yes
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "openwith"; Description: "List Chippy under ""Open with"" for chiptunes, modules and other music (does not change your default player)"; GroupDescription: "Files:"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The program, for "Open with" (and for `start chippy` from Run).
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExe}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; The file type Chippy offers to open, without taking any extension over.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "Music"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: openwith

; One OpenWithProgids value per extension (kept in step with
; crates/chippy-core/src/formats.rs; vgmstream's hundreds are left out bar
; the common ones). Removed again on uninstall, leaving each extension's
; other handlers alone.
#define Exts "nsf nsfe gbs hes kss ay sap spc vgm vgz gym usf miniusf psf minipsf psf2 minipsf2 ssf minissf dsf minidsf qsf miniqsf spu sid psid rsid sgc gsf minigsf 2sf mini2sf wsr mod xm s3m it mtm 669 med okt stm far ult ptm dbm amf mdl psm gdm imf umx mid midi kar rmi adx hca brstm bcstm bfstm bwav xwb fsb at3 dsp mp3 flac ogg opus wav m4a cue chd gdi ccd "
#define Rest Exts
#define Ext ""
#sub AddExt
  #define public Ext Copy(Rest, 1, Pos(" ", Rest) - 1)
  #define public Rest Copy(Rest, Pos(" ", Rest) + 1)
Root: HKA; Subkey: "Software\Classes\.{#Ext}\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".{#Ext}"; ValueData: ""
#endsub
#for {0; Len(Rest) > 0; 0} AddExt

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// After the program is gone, offer to delete the settings too. No by
// default, so reinstalling keeps them; never asked (and kept) when the
// uninstall is silent.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Settings, Cache: String;
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent() then
  begin
    // Where Chippy keeps them (the directories crate's places for "chippy").
    Settings := ExpandConstant('{userappdata}\chippy');
    Cache := ExpandConstant('{localappdata}\chippy');
    if DirExists(Settings) or DirExists(Cache) then
      if SuppressibleMsgBox(
           'Also delete your Chippy settings?' + #13#10#13#10 +
           'This removes your saved settings, playlist and window layout' +
           ' (' + Settings + '). Your music is not touched.' + #13#10#13#10 +
           'Keep them if you might install Chippy again.',
           mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      begin
        DelTree(Settings, True, True, True);
        DelTree(Cache, True, True, True);
      end;
  end;
end;
