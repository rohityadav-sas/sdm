# Smart Download Manager (SDM) installer for Windows (PowerShell 5.1+)
#   irm https://raw.githubusercontent.com/rohityadav-sas/sdm/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = 'rohityadav-sas/sdm'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\sdm'
$InstallDir = [IO.Path]::GetFullPath($InstallDir)

$cpu = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$arch = switch ($cpu) {
    'AMD64' { 'x86_64' }
    'ARM64' { 'arm64' }
    default { throw "Unsupported architecture: $cpu" }
}

$api = "https://api.github.com/repos/$Repo/releases/latest"

Write-Host "Looking up latest release..."
try {
    $release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'sdm-installer' }
    $asset = $release.assets | Where-Object { $_.name -like "*windows-$arch.zip" -or $_.name -like "*windows*.zip" } | Select-Object -First 1
    if (-not $asset) { throw "No release asset found for windows-$arch" }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("sdm-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp 'sdm.zip'
        Write-Host "Downloading $($asset.name)..."
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip -UseBasicParsing
        Expand-Archive -Path $zip -DestinationPath (Join-Path $tmp 'out') -Force

        $exe = Get-ChildItem -Path (Join-Path $tmp 'out') -Filter 'sdm.exe' -Recurse | Select-Object -First 1
        if (-not $exe) { throw "sdm.exe was not found inside the archive" }

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item -LiteralPath $exe.FullName -Destination (Join-Path $InstallDir 'sdm.exe') -Force

        # Also copy companion host helper if present
        $helper = Get-ChildItem -Path (Join-Path $tmp 'out') -Filter 'sdm-host.exe' -Recurse | Select-Object -First 1
        if ($helper) {
            Copy-Item -LiteralPath $helper.FullName -Destination (Join-Path $InstallDir 'sdm-host.exe') -Force
        }
    }
    finally {
        Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
catch {
    Write-Warning "Could not download prebuilt release ($($_.Exception.Message))."
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "cargo detected. Building and installing from source..."
        $tmpSrc = Join-Path ([IO.Path]::GetTempPath()) ("sdm-src-" + [guid]::NewGuid().ToString('N'))
        git clone --depth 1 "https://github.com/$Repo.git" $tmpSrc
        Push-Location $tmpSrc
        try {
            cargo build --release
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
            Copy-Item -Path 'target\release\sdm.exe' -Destination (Join-Path $InstallDir 'sdm.exe') -Force
            if (Test-Path 'target\release\sdm-host.exe') {
                Copy-Item -Path 'target\release\sdm-host.exe' -Destination (Join-Path $InstallDir 'sdm-host.exe') -Force
            }
        }
        finally {
            Pop-Location
            Remove-Item -Path $tmpSrc -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    else {
        throw $_
    }
}

Write-Host "Installed sdm to $InstallDir\sdm.exe"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = if ($userPath) { $userPath -split ';' | Where-Object { $_ } } else { @() }
if ($entries -notcontains $InstallDir) {
    $newPath = (@($entries) + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to your PATH. Restart other terminals to pick it up."
}
Write-Host "Run: sdm"

