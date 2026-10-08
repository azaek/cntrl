# Installs the cntrl agent on Windows and enrolls it with Console (D58).
# Console serves this script with the release's downloads and SHA-256s set
# above it: with a single-use token for Add device's command, or without one
# for any machine, which enrolls afterwards. Run it from PowerShell opened as
# administrator:
#
#   irm https://gw.cntrl.pw/install/<token>.ps1 | iex
#   irm https://gw.cntrl.pw/install.ps1 | iex
#
# On a machine that's in another account already, a token moves it there
# after asking; with $env:CNTRL_MOVE = 1 set first, it moves it without
# asking (D23). $env:CNTRL_FORCE = 1 installs Console's release even when it,
# or a newer agent, is installed.
#
# The download is checked against the SHA-256 Console verified from the
# release's signed manifest, then cntrl-agent.exe installs itself: under
# Program Files, as two services, with its data in %ProgramData%\cntrl. Running
# Console's command again updates the agent when the release is newer, and
# otherwise only makes sure it runs; either way the agent keeps its identity.
# Everything runs inside one script block, called on the last line, so a
# download cut short runs nothing, and a failure never closes the window.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    if (-not (Get-Variable CNTRL_CONSOLE -ValueOnly -ErrorAction SilentlyContinue)) { $CNTRL_CONSOLE = 'https://gw.cntrl.pw' }
    foreach ($name in 'CNTRL_TOKEN', 'CNTRL_GATEWAY', 'CNTRL_VERSION', 'CNTRL_ARTIFACTS') {
        if (-not (Get-Variable $name -ErrorAction SilentlyContinue)) { Set-Variable $name '' }
    }

    function Say([string]$Text) { Write-Host $Text }
    function Fail([string]$Text) { throw "cntrl: $Text" }

    # Runs a program for its exit code alone. Windows PowerShell turns a
    # redirected stderr into errors, which 'Stop' would end the script on.
    function Quietly([string]$Program, [string[]]$Arguments) {
        $ErrorActionPreference = 'Continue'
        & $Program @Arguments *> $null
        return $LASTEXITCODE
    }

    # A program's output, both streams, as one string.
    function OutputOf([string]$Program, [string[]]$Arguments) {
        $ErrorActionPreference = 'Continue'
        return (& $Program @Arguments 2>&1 | Out-String)
    }

    # A version's parts as numbers, a prerelease before its release.
    function Older([string]$A, [string]$B) {
        $a, $aPre = $A -split '-', 2
        $b, $bPre = $B -split '-', 2
        $as = $a -split '\.' | ForEach-Object { [int64]$_ }
        $bs = $b -split '\.' | ForEach-Object { [int64]$_ }
        for ($i = 0; $i -lt 3; $i++) {
            if ($as[$i] -ne $bs[$i]) { return $as[$i] -lt $bs[$i] }
        }
        if ($aPre -and -not $bPre) { return $true }
        if ($bPre -and -not $aPre) { return $false }
        return [string]::CompareOrdinal($aPre, $bPre) -lt 0
    }

    # The installed agent's version, such as 0.1.15; nothing when there's none.
    function VersionOf([string]$Bin) {
        if (-not (Test-Path -LiteralPath $Bin)) { return '' }
        $said = & $Bin --version
        if ($LASTEXITCODE -ne 0 -or -not $said) { return '' }
        return ("$said".Trim() -split ' ')[-1]
    }

    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Fail 'open PowerShell as administrator, then run the command again'
    }
    # Windows PowerShell 5.1 may still offer TLS 1.0 first.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    # Smart App Control, while the agent is unsigned (D92): when it's on, it
    # blocks an unsigned app each time it starts, with no exception for one
    # app, so nothing is installed until it's off. Evaluating, it blocks
    # nothing yet. Windows 11 keeps its state here: 0 off, 1 on, 2 evaluating;
    # older Windows has none.
    $sac = (Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -Name VerifiedAndReputablePolicyState -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
    if ($sac -eq 1) {
        Fail ('Smart App Control is on, and it blocks the cntrl agent until the agent is signed. ' +
            'Turn it off in Windows Security, under App & browser control, then run the command again. Leave it off until the agent is signed.')
    }
    if ($sac -eq 2) {
        Say 'Smart App Control is evaluating this PC. If it turns on, it blocks the cntrl agent until the agent is signed: turn it off then, in Windows Security, under App & browser control.'
    }

    # The machine's own architecture, not this PowerShell's, which may run
    # emulated.
    $machine = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE
    $target = switch ($machine) {
        'AMD64' { 'x86_64-pc-windows-msvc' }
        'ARM64' { 'aarch64-pc-windows-msvc' }
        default { Fail "there are no Windows releases for $machine" }
    }
    $bin = Join-Path $env:ProgramFiles 'cntrl\cntrl-agent.exe'
    $had = VersionOf $bin

    $keep = $had -and $CNTRL_VERSION -and -not $env:CNTRL_FORCE -and -not (Older $had $CNTRL_VERSION)
    if ($keep) {
        if ($had -eq $CNTRL_VERSION) {
            Say "cntrl agent $had, the latest release, is installed already."
        } else {
            Say "Keeping cntrl agent $had, which is newer than the latest release, $CNTRL_VERSION."
        }
        Start-Service cntrl-privd, cntrl-agent
    } else {
        if (-not $CNTRL_ARTIFACTS) { Fail "nothing to install: use the command from Console's Add device dialog" }
        $line = $CNTRL_ARTIFACTS -split "`n" | Where-Object { ($_ -split ' ')[0] -eq $target } | Select-Object -First 1
        if (-not $line) { Fail "release $CNTRL_VERSION has no build for $target" }
        $null, $url, $size, $want = $line -split ' '
        # A folder only administrators can change, so nothing swaps the files
        # between checking and running them.
        $work = Join-Path $env:ProgramData "cntrl-install-$([guid]::NewGuid())"
        New-Item -ItemType Directory -Path $work | Out-Null
        try {
            $zip = Join-Path $work 'agent.zip'
            Say ('Downloading cntrl agent {0} for {1} ({2:N1} MB)' -f $CNTRL_VERSION, $target, ([int64]$size / 1MB))
            Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
            $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($got -ne $want) { Fail "the download doesn't match the release's SHA-256 (got $got)" }
            Expand-Archive -LiteralPath $zip -DestinationPath (Join-Path $work 'unpacked')
            $new = Get-ChildItem -LiteralPath (Join-Path $work 'unpacked') -Recurse -Filter 'cntrl-agent.exe' | Select-Object -First 1
            if (-not $new) { Fail 'the archive has no cntrl-agent.exe' }
            $arguments = @('install', '--console', $CNTRL_CONSOLE)
            if ($CNTRL_GATEWAY) { $arguments += @('--gateway', $CNTRL_GATEWAY) }
            & $new.FullName @arguments
            if ($LASTEXITCODE -ne 0) { Fail 'the agent could not install itself; see above' }
        } finally {
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    # Waits for the agent, then hands it the token, if there is one. The agent
    # enrolls, says the machine is in that account already, or asks before
    # moving it from another (D23).
    for ($tries = 0; ; $tries++) {
        if ((Quietly $bin @('status')) -eq 0) { break }
        if ($tries -ge 20) { Fail "the agent didn't start; its log is in $env:ProgramData\cntrl\logs" }
        Start-Sleep -Milliseconds 500
    }
    if ($CNTRL_TOKEN) {
        $env:CNTRL_INSTALLER = '1'
        try {
            if ($env:CNTRL_MOVE) { $CNTRL_TOKEN | & $bin enroll --move } else { $CNTRL_TOKEN | & $bin enroll }
            if ($LASTEXITCODE -ne 0) { Fail 'enrolling did not finish; see above' }
        } finally {
            Remove-Item Env:\CNTRL_INSTALLER -ErrorAction SilentlyContinue
        }
    } elseif ((OutputOf $bin @('status')) -match '(?m)^uplink: not enrolled') {
        Say "To add this machine, run the command from Console's Add device dialog."
    } else {
        Say 'This machine is enrolled already.'
    }
}
