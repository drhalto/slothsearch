param([switch]$Uninstall)
# Builds SlothSearch (release) and installs it to %LOCALAPPDATA%\Programs\SlothSearch, set
# to start at login. Run again after changes to update. -Uninstall removes the program and
# the login entry and Start menu shortcut but keeps your settings and history in %APPDATA%\SlothSearch.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $env:LOCALAPPDATA "Programs\SlothSearch"
$run = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "SlothSearch.lnk"

Get-Process slothsearch -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

if ($Uninstall) {
    Remove-ItemProperty $run -Name SlothSearch -ErrorAction SilentlyContinue
    Remove-Item $lnk -Force -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    "Uninstalled. Settings remain in $env:APPDATA\SlothSearch."
    return
}

cargo build --release --manifest-path (Join-Path $root "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "build failed" }

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
