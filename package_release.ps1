param(
    [switch]$SkipTests,
    [switch]$NoZip,
    [string]$TargetTriple = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-ExternalCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [string[]]$Arguments = @()
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "命令执行失败: $FilePath $($Arguments -join ' ')"
    }
}

function Resolve-CargoExecutable {
    $cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
    if ($null -ne $cargoCommand -and -not [string]::IsNullOrWhiteSpace($cargoCommand.Source)) {
        return $cargoCommand.Source
    }

    $cargoFallback = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $cargoFallback) {
        return $cargoFallback
    }

    throw "未找到 cargo，可将 Rust 工具链加入 PATH，或确认 $cargoFallback 存在"
}

function Get-PackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CargoTomlPath
    )

    $cargoText = Get-Content -Raw -Encoding UTF8 $CargoTomlPath
    $match = [regex]::Match($cargoText, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $match.Success) {
        throw "无法从 Cargo.toml 解析版本号"
    }

    return $match.Groups[1].Value
}

function Get-RelativeOutputPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BasePath,

        [Parameter(Mandatory = $true)]
        [string]$TargetPath
    )

    $resolvedBase = (Resolve-Path $BasePath).Path
    $resolvedTarget = (Resolve-Path $TargetPath).Path
    return [System.IO.Path]::GetRelativePath($resolvedBase, $resolvedTarget)
}

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot

try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $cargoExecutable = Resolve-CargoExecutable

    $version = Get-PackageVersion -CargoTomlPath (Join-Path $scriptRoot "Cargo.toml")
    $projectName = "copilot-stop-notif"
    $bundleLabel = if ([string]::IsNullOrWhiteSpace($TargetTriple)) { "windows-x64" } else { $TargetTriple }
    $bundleName = "$projectName-v$version-$bundleLabel"
    $distRoot = Join-Path $scriptRoot "dist"
    $bundleDir = Join-Path $distRoot $bundleName
    $zipPath = Join-Path $distRoot "$bundleName.zip"
    $exeName = "$projectName.exe"
    $hookConfigName = "$projectName.json"
    $envExampleName = "$projectName.env.example"

    New-Item -ItemType Directory -Force -Path $distRoot | Out-Null

    if (-not $SkipTests) {
        Invoke-ExternalCommand -FilePath $cargoExecutable -Arguments @("test")
    }

    $buildArgs = @("build", "--release")
    if (-not [string]::IsNullOrWhiteSpace($TargetTriple)) {
        $buildArgs += @("--target", $TargetTriple)
    }
    Invoke-ExternalCommand -FilePath $cargoExecutable -Arguments $buildArgs

    $releaseDir = if ([string]::IsNullOrWhiteSpace($TargetTriple)) {
        Join-Path $scriptRoot "target\release"
    } else {
        Join-Path $scriptRoot ("target\{0}\release" -f $TargetTriple)
    }

    $builtExePath = Join-Path $releaseDir $exeName
    if (-not (Test-Path $builtExePath)) {
        throw "未找到编译产物: $builtExePath"
    }

    $hookDir = Join-Path $scriptRoot ".github\hooks"

    if (Test-Path $bundleDir) {
        Remove-Item -Recurse -Force $bundleDir
    }
    if (Test-Path $zipPath) {
        Remove-Item -Force $zipPath
    }

    $bundleHookDir = Join-Path $bundleDir ".github\hooks"
    $bundleHookAssetDir = Join-Path $bundleHookDir $projectName
    New-Item -ItemType Directory -Force -Path $bundleHookAssetDir | Out-Null
    $bundleExePath = Join-Path $bundleHookAssetDir $exeName
    $bundleEnvExamplePath = Join-Path $bundleHookAssetDir $envExampleName

    Copy-Item -Force (Join-Path $hookDir $hookConfigName) (Join-Path $bundleHookDir $hookConfigName)
    Copy-Item -Force $builtExePath $bundleExePath
    Copy-Item -Force (Join-Path $hookDir $envExampleName) $bundleEnvExamplePath
    if (Test-Path (Join-Path $scriptRoot "README.md")) {
        Copy-Item -Force (Join-Path $scriptRoot "README.md") (Join-Path $bundleDir "README.md")
    }

    if (-not $NoZip) {
        Compress-Archive -Path $bundleDir -DestinationPath $zipPath -Force
    }

    [pscustomobject]@{
        ok = $true
        version = $version
        hookExe = Get-RelativeOutputPath -BasePath $scriptRoot -TargetPath $bundleExePath
        bundleDir = Get-RelativeOutputPath -BasePath $scriptRoot -TargetPath $bundleDir
        zipPath = $(if ((-not $NoZip) -and (Test-Path $zipPath)) { Get-RelativeOutputPath -BasePath $scriptRoot -TargetPath $zipPath } else { "" })
    } | ConvertTo-Json -Compress
}
finally {
    Pop-Location
}