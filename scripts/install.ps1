param([switch]$Uninstall, [switch]$Force)
# Builds SlothSearch (release) and installs it to %LOCALAPPDATA%\Programs\SlothSearch, set
# to start at login. Run again after changes to update. -Uninstall removes the program and
# the login entry and Start menu shortcut but keeps your settings and history in %APPDATA%\SlothSearch.
# An update won't stop SlothSearch while a snip, recording or color pick is open; -Force does.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $env:LOCALAPPDATA "Programs\SlothSearch"
$run = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "SlothSearch.lnk"

function Stop-SlothSearch {
    Get-Process slothsearch -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

if ($Uninstall) {
    Stop-SlothSearch
    Remove-ItemProperty $run -Name SlothSearch -ErrorAction SilentlyContinue
    Remove-Item $lnk -Force -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    "Uninstalled. Settings remain in $env:APPDATA\SlothSearch."
    return
}

# Build first, so a failed build leaves the installed copy running.
cargo build --release --manifest-path (Join-Path $root "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "build failed" }

# Stopping it would throw away an unsaved snip or cut a recording short.
if (-not $Force) {
    if (-not ("SlothSearchInstall.Win" -as [type])) {
        Add-Type -Namespace SlothSearchInstall -Name Win -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);'
    }
    $busy = "SlothSearchSnipPreview", "SlothSearchSnip", "SlothSearchRecordBar", "SlothSearchColorPicker" |
        Where-Object { [SlothSearchInstall.Win]::FindWindowW($_, [NullString]::Value) -ne [IntPtr]::Zero }
    if ($busy) { throw "SlothSearch has a snip, recording or color pick open. Finish it and run this again, or pass -Force." }
}
Stop-SlothSearch

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $root "target\release\slothsearch.exe") (Join-Path $dest "slothsearch.exe") -Force
Set-ItemProperty $run -Name SlothSearch -Value ('"' + (Join-Path $dest "slothsearch.exe") + '"')
# Start menu entry, so it can be found and relaunched after quitting from the tray.
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$s.TargetPath = Join-Path $dest "slothsearch.exe"
$s.WorkingDirectory = $dest
$s.Description = "Spotlight-style launcher (Alt+Space)"
$s.Save()
Start-Process (Join-Path $dest "slothsearch.exe")
"Installed to $dest and set to start at login. Press Alt+Space."
