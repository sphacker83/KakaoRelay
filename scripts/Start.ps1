$ErrorActionPreference = 'Stop'
$relayRoot = Split-Path -Parent $PSScriptRoot
$relayTarget = Join-Path $relayRoot 'artifacts\app'
$relayExecutable = Join-Path $relayTarget 'KakaoRelay.exe'
$relayStage = Join-Path $relayRoot 'artifacts\model-catalog-ready'
$relayPending = Join-Path $relayRoot 'artifacts\model-catalog-update.pending'
# The application owns this mutex for its whole lifetime. Updating while it is
# held would disrupt active reply sessions, so only the next cold start applies it.
$relayGate = [Threading.Mutex]::new($false, 'Local\KakaoRelay.MainWindow')
$relayOwnsGate = $false
$relayLogFolder = Join-Path $env:LOCALAPPDATA 'KakaoRelay\logs'
[IO.Directory]::CreateDirectory($relayLogFolder) | Out-Null
$relayLog = Join-Path $relayLogFolder 'startup.log'
function Write-StartupLog([string]$message) {
    Add-Content -LiteralPath $relayLog -Value ('{0:o} {1}' -f [DateTimeOffset]::Now, $message) -Encoding UTF8
}
Write-StartupLog 'Launcher started.'
try {
    try { $relayOwnsGate = $relayGate.WaitOne(0) } catch [Threading.AbandonedMutexException] { $relayOwnsGate = $true }
    if ($relayOwnsGate -and (Test-Path -LiteralPath $relayPending)) {
        $relayExpected = (Get-Content -Raw -LiteralPath $relayPending).Trim()
        if ($relayExpected -notmatch '^[0-9A-Fa-f]{64}$') { throw 'Invalid update checksum. Update was not applied.' }
        Write-Host 'Applying KakaoRelay update. Please wait...'; Write-StartupLog 'Applying pending update.'
        $relayActual = (Get-FileHash -LiteralPath (Join-Path $relayStage 'KakaoRelay.exe') -Algorithm SHA256).Hash
        if ($relayActual -ne $relayExpected) { throw 'Update payload verification failed.' }
        $relayTargetFull = [IO.Path]::GetFullPath($relayTarget) + [IO.Path]::DirectorySeparatorChar
        foreach ($relayFile in Get-ChildItem -LiteralPath $relayStage -File -Recurse) {
            $relayRelative = $relayFile.FullName.Substring($relayStage.Length).TrimStart('\')
            if ($relayRelative -match '^(data|logs)(\\|$)') { continue }
            $relayDestination = [IO.Path]::GetFullPath((Join-Path $relayTarget $relayRelative))
            if (-not $relayDestination.StartsWith($relayTargetFull, [StringComparison]::OrdinalIgnoreCase)) { throw 'Update target escaped the app directory.' }
            [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($relayDestination)) | Out-Null
            Copy-Item -LiteralPath $relayFile.FullName -Destination $relayDestination -Force
        }
        Remove-Item -LiteralPath $relayPending
        Write-StartupLog 'Update applied.'
    }
} catch {
    Write-StartupLog ('Startup update failed: ' + $_.Exception.Message)
    throw
} finally {
    if ($relayOwnsGate) { $relayGate.ReleaseMutex() }
    $relayGate.Dispose()
}
try {
    if (-not (Test-Path -LiteralPath $relayExecutable -PathType Leaf)) { throw 'KakaoRelay.exe is missing. Build or restore the app first.' }
    $relayProcess = Start-Process -FilePath $relayExecutable -WorkingDirectory $relayTarget -ArgumentList '--resume-replies' -WindowStyle Normal -PassThru
    Write-StartupLog ('App launched or existing window requested. PID=' + $relayProcess.Id)
} catch {
    Write-StartupLog ('App launch failed: ' + $_.Exception.Message)
    throw
}
