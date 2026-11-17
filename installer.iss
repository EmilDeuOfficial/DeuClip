#define MyAppName "DeuClip"
; Read from the version resource of the EXE (version in Cargo.toml).
#define MyAppVersion GetStringFileInfo(AddBackslash(SourcePath) + "dist\DeuClip.exe", "ProductVersion")
#if MyAppVersion == ""
  #error dist\DeuClip.exe has no version resource, please build it with build.py
#endif
#define MyAppPublisher "EmilDeuOfficial"
#define MyAppExeName "DeuClip.exe"
#define MyAppURL "https://github.com/EmilDeuOfficial/DeuClip"
; Windows build of FFmpeg with ddagrab (always the latest version).
#define FFmpegZipUrl "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip"

[Setup]
AppId={{B7C4E2A1-5D3F-4E8A-9C61-2F7A8D0E4B13}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
VersionInfoVersion={#GetVersionNumbersString(AddBackslash(SourcePath) + "dist\DeuClip.exe")}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=dist
OutputBaseFilename=DeuClip_Setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; Per user without administrator rights by default, for all users on request.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
UninstallDisplayIcon={app}\{#MyAppExeName}
SetupIconFile=img\app.ico
MinVersion=10.0
CloseApplications=yes
RestartApplications=no
; Writes "Setup Log <date>.txt" to %TEMP%, useful when FFmpeg could not be downloaded.
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german";  MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
english.FFmpegDownloadTitle=Downloading FFmpeg
english.FFmpegDownloadDesc=DeuClip needs FFmpeg to record. The setup downloads it once (about 190 MB).
english.FFmpegMemo=FFmpeg: will be downloaded automatically
english.FFmpegFailed=FFmpeg could not be installed automatically.%nPlease place ffmpeg.exe next to DeuClip.exe or in your PATH after setup.%nDownload: https://ffmpeg.org/download.html
english.FFmpegInstalling=Installing FFmpeg...
german.FFmpegDownloadTitle=FFmpeg wird heruntergeladen
german.FFmpegDownloadDesc=DeuClip braucht FFmpeg für die Aufnahme. Das Setup lädt es einmalig herunter (ca. 190 MB).
german.FFmpegMemo=FFmpeg: wird automatisch heruntergeladen
german.FFmpegFailed=FFmpeg konnte nicht automatisch installiert werden.%nBitte lege ffmpeg.exe nach dem Setup neben DeuClip.exe oder nimm es in den PATH auf.%nDownload: https://ffmpeg.org/download.html
german.FFmpegInstalling=FFmpeg wird installiert...

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "dist\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[UninstallDelete]
; FFmpeg that this setup placed next to the EXE
Type: files; Name: "{app}\ffmpeg.exe"

[Icons]
Name: "{group}\{#MyAppName}";           Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}";     Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
var
  DownloadPage: TDownloadWizardPage;
  FFmpegDownloaded: Boolean;

function FFmpegInPath(): Boolean;
var
  ResultCode: Integer;
begin
  Result := Exec(ExpandConstant('{cmd}'), '/c where ffmpeg >nul 2>&1', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := Result and (ResultCode = 0);
end;

function NeedFFmpeg(): Boolean;
begin
  Result := not FileExists(ExpandConstant('{app}\ffmpeg.exe')) and not FFmpegInPath();
end;

function OnDownloadProgress(const Url, FileName: String; const Progress, ProgressMax: Int64): Boolean;
begin
  if Progress = ProgressMax then
    Log(Format('Download finished: %s', [FileName]));
  Result := True;
end;

procedure InitializeWizard();
begin
  DownloadPage := CreateDownloadPage(CustomMessage('FFmpegDownloadTitle'), CustomMessage('FFmpegDownloadDesc'), @OnDownloadProgress);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = wpReady) and NeedFFmpeg() then begin
    DownloadPage.Clear;
    DownloadPage.Add('{#FFmpegZipUrl}', 'ffmpeg.zip', '');
    DownloadPage.Show;
    try
      try
        DownloadPage.Download;
        FFmpegDownloaded := True;
      except
        // The installation continues anyway, the notice follows at the end.
        Log('FFmpeg download failed: ' + GetExceptionMessage);
      end;
    finally
      DownloadPage.Hide;
    end;
  end;
end;

// Extracts only bin\ffmpeg.exe from the archive into {app}.
function ExtractFFmpeg(): Boolean;
var
  ResultCode: Integer;
  ZipFile, DestFile: String;
begin
  ZipFile := ExpandConstant('{tmp}\ffmpeg.zip');
  DestFile := ExpandConstant('{app}\ffmpeg.exe');
  Exec(
    'powershell.exe',
    '-NoProfile -ExecutionPolicy Bypass -Command "Add-Type -AssemblyName System.IO.Compression.FileSystem; ' +
    '$z = [IO.Compression.ZipFile]::OpenRead(''' + ZipFile + '''); ' +
    'try { $e = $z.Entries | Where-Object { $_.FullName -like ''*/bin/ffmpeg.exe'' } | Select-Object -First 1; ' +
    '[IO.Compression.ZipFileExtensions]::ExtractToFile($e, ''' + DestFile + ''', $true) } finally { $z.Dispose() }"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode
  );
  Result := FileExists(DestFile);
  if not Result then
    Log(Format('FFmpeg extraction failed (exit code %d)', [ResultCode]));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    if FFmpegDownloaded then begin
      WizardForm.StatusLabel.Caption := CustomMessage('FFmpegInstalling');
      if not ExtractFFmpeg() then
        SuppressibleMsgBox(CustomMessage('FFmpegFailed'), mbInformation, MB_OK, IDOK);
    end else if NeedFFmpeg() then
      SuppressibleMsgBox(CustomMessage('FFmpegFailed'), mbInformation, MB_OK, IDOK);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then begin
    // Remove the autostart entry of the app
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'DeuClip');
    // Remove configuration and log file
    DelTree(ExpandConstant('{userappdata}\DeuClip'), True, True, True);
  end;
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo,
  MemoTypeInfo, MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
var
  S: String;
begin
  S := '';
  if MemoDirInfo <> '' then S := S + MemoDirInfo + NewLine + NewLine;
  if MemoGroupInfo <> '' then S := S + MemoGroupInfo + NewLine + NewLine;
  if MemoTasksInfo <> '' then S := S + MemoTasksInfo + NewLine + NewLine;
  if NeedFFmpeg() then
    S := S + CustomMessage('FFmpegMemo') + NewLine;
  Result := S;
end;
