# penv installer. irm https://penv.cloud/install.ps1 | iex
#
# Flags when the file is run directly: -Version v1.2.3, -InstallDir <path>,
# -ReleaseBase <url>, -AddToPath. Piped, there are no flags, so the same four read as
# the environment variables PENV_VERSION, PENV_INSTALL_DIR, PENV_RELEASE_BASE and
# PENV_ADD_TO_PATH=1, which is the only way to ask for the PATH entry through irm | iex.
# PENV_ALLOW_ELEVATED=1 installs from an elevated shell; the default refuses.

# A script block so nothing here leaks into the session that piped it in.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    $targets = 'x86_64-unknown-linux-musl, aarch64-unknown-linux-musl, x86_64-apple-darwin, aarch64-apple-darwin, x86_64-pc-windows-msvc, aarch64-pc-windows-msvc'

    # The release keys, base64 Ed25519 public keys separated by spaces, that a checksum file
    # may be signed with: the same list install.sh carries in public_keys.
    $publicKeys = 'VRJ90W7uzjrwQKeD6KCGQj1dih6z6/4QVeat0M5qL/0='

    $version = $env:PENV_VERSION
    $installDir = $env:PENV_INSTALL_DIR
    $releaseBase = $env:PENV_RELEASE_BASE
    $addToPath = $env:PENV_ADD_TO_PATH -eq '1'

    $rest = @($args)
    $i = 0
    while ($i -lt $rest.Count) {
        $flag = $rest[$i] -replace '^-{1,2}', ''
        if ($flag -eq 'AddToPath') { $addToPath = $true; $i++; continue }
        if (@('Version', 'InstallDir', 'ReleaseBase') -notcontains $flag) {
            throw "penv: $($rest[$i]) is not a flag this installer takes."
        }
        if ($i + 1 -ge $rest.Count) { throw "penv: $($rest[$i]) needs a value." }
        if ($flag -eq 'Version') { $version = $rest[$i + 1] }
        elseif ($flag -eq 'InstallDir') { $installDir = $rest[$i + 1] }
        else { $releaseBase = $rest[$i + 1] }
        $i += 2
    }

    if ((Test-Path Variable:IsWindows) -and -not $IsWindows) {
        throw 'penv: on macOS and Linux run: curl -fsSL https://penv.cloud/install | sh'
    }
    $me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if ($me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -and $env:PENV_ALLOW_ELEVATED -ne '1') {
        throw 'penv: this installs into your profile, so run it in a normal PowerShell rather than an elevated one, or set $env:PENV_ALLOW_ELEVATED = ''1''.'
    }

    # The process variables say AMD64 to an x64 shell on an ARM64 machine, so the machine's own value is read.
    $machine = [Microsoft.Win32.Registry]::GetValue(
        'HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
        'PROCESSOR_ARCHITECTURE', $null)
    if (-not $machine) {
        $machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    $arch = switch ($machine) {
        'AMD64' { 'x86_64' }
        'ARM64' { 'aarch64' }
        default { throw "penv: no build for $machine on Windows. The releases carry $targets." }
    }
    $triple = "$arch-pc-windows-msvc"

    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    # penv.cloud redirects to whichever host serves the releases, so no repository path lives here.
    if (-not $releaseBase) { $releaseBase = 'https://penv.cloud' }
    $releaseBase = $releaseBase.TrimEnd('/')

    if (-not $version) {
        $latest = "$releaseBase/releases/latest"
        try { $body = (Invoke-WebRequest -Uri $latest -UseBasicParsing -Headers @{ Accept = 'application/json' }).Content }
        catch { throw "penv: $latest could not be read. Check the network and try again." }
        # Bytes rather than text whenever the answer does not call itself json.
        if ($body -is [byte[]]) { $body = [Text.Encoding]::UTF8.GetString($body) }
        try { $version = ($body | ConvertFrom-Json).tag_name }
        catch { throw "penv: $latest answered something that is not a release." }
        if (-not $version) { throw 'penv: the latest release carries no tag_name. Try again later.' }
    }
    # One shape whichever way the tag arrived, and nothing in it that could reach past the asset.
    $version = 'v' + ($version -replace '^v', '')
    if ($version -match '[/\\\s]') {
        throw "penv: $version is not a tag such as v1.2.3: a tag carries no slash and no whitespace."
    }

    $asset = "penv-$version-$triple.exe"
    $sums = "penv-$version-$triple.sha256"
    $download = "$releaseBase/releases/download/$version"
    if (-not $installDir) { $installDir = Join-Path $env:USERPROFILE '.penv\bin' }
    # Absolute, so the PATH entry means the same folder from every shell.
    $installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($installDir)

    # Ed25519 arrived in OpenSSL 1.1.1, and only OpenSSL itself takes -rawin; LibreSSL does not.
    function OpensslThatVerifies {
        $command = Get-Command openssl -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $command) { return $null }
        # 'Continue' locally, so a native command writing to stderr is not a thrown error in PowerShell 5.1.
        $ErrorActionPreference = 'Continue'
        $said = (& $command.Source version 2>$null | Out-String).Trim()
        if ($said -notmatch '^OpenSSL (\d+)\.(\d+)\.(\d+)') { return $null }
        $major = [int]$Matches[1]; $minor = [int]$Matches[2]; $patch = [int]$Matches[3]
        if ($major -gt 1 -or ($major -eq 1 -and ($minor -gt 1 -or ($minor -eq 1 -and $patch -ge 1)))) {
            return $command.Source
        }
        return $null
    }

    # The 12 SPKI header bytes are a multiple of three, so the PEM body is that
    # header's base64 followed by the key's own, unchanged.
    function SignedByAReleaseKey($openssl, $keys, $file, $sigFile, $scratch) {
        # The signature is one long base64 line, whatever ended it.
        try { $bytes = [Convert]::FromBase64String(([IO.File]::ReadAllText($sigFile) -replace '\s', '')) }
        catch { return $false }
        $signature = Join-Path $scratch 'signature.bin'
        [IO.File]::WriteAllBytes($signature, $bytes)
        $pem = Join-Path $scratch 'key.pem'
        $ErrorActionPreference = 'Continue'
        foreach ($key in $keys) {
            # 32 raw bytes are 44 base64 characters, so a key pasted without its
            # trailing = is the same key and gets it back.
            if ($key.Length -eq 43) { $key = "$key=" }
            [IO.File]::WriteAllText($pem, "-----BEGIN PUBLIC KEY-----`nMCowBQYDK2VwAyEA$key`n-----END PUBLIC KEY-----`n")
            & $openssl pkeyutl -verify -pubin -inkey $pem -rawin -in $file -sigfile $signature 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) { return $true }
        }
        return $false
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ("penv-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    try {
        Write-Host "penv $version for $triple"
        foreach ($name in @($asset, $sums)) {
            try { Invoke-WebRequest -Uri "$download/$name" -OutFile (Join-Path $work $name) -UseBasicParsing }
            catch { throw "penv: $download/$name could not be downloaded." }
        }

        # .NET carries no Ed25519, so OpenSSL on PATH verifies the signature, as in install.sh.
        # The signature stands in front of the digest: an unsigned checksum file says
        # nothing about the binary it lists.
        $keys = @($publicKeys -split '\s+' | Where-Object { $_ })
        if ($keys.Count -eq 0) {
            Write-Host 'signature not checked: this installer carries no release key' -ForegroundColor DarkGray
        }
        elseif (-not ($openssl = OpensslThatVerifies)) {
            Write-Host 'signature not checked: OpenSSL 1.1.1 or newer verifies it, and this host has none' -ForegroundColor DarkGray
        }
        else {
            $sig = Join-Path $work "$sums.sig"
            try { Invoke-WebRequest -Uri "$download/$sums.sig" -OutFile $sig -UseBasicParsing }
            catch { throw "penv: $download/$sums.sig could not be downloaded, and a release is signed. Nothing was installed." }
            if (-not (SignedByAReleaseKey $openssl $keys (Join-Path $work $sums) $sig $work)) {
                throw "penv: $sums is not signed by a penv release key. Nothing was installed."
            }
        }

        # The checksum file covers the archive too, so the raw binary's line is matched whole.
        $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [Regex]::Escape($asset) + '$'
        $line = Get-Content (Join-Path $work $sums) | Where-Object { $_ -match $pattern } | Select-Object -First 1
        if (-not $line) { throw "penv: $sums lists no sha256 digest for $asset." }
        $null = $line -match $pattern
        $expected = $Matches[1]
        $actual = (Get-FileHash -Path (Join-Path $work $asset) -Algorithm SHA256).Hash
        if ($expected -ne $actual) {
            throw "penv: $asset is not the file $sums names. Nothing was installed."
        }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        $binary = Join-Path $installDir 'penv.exe'
        try { Move-Item -Path (Join-Path $work $asset) -Destination $binary -Force }
        catch { throw "penv: $binary could not be written. Close any running penv and try again." }
    }
    finally { Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue }

    Write-Host "installed $binary"

    $onPath = @($env:Path -split ';') -contains $installDir
    if ($addToPath) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        $added = $false
        try {
            # Raw and unexpanded, and written back as the kind it already was, so a PATH holding
            # %USERPROFILE% keeps the variable rather than this machine's answer to it.
            $has = @($key.GetValueNames()) -contains 'Path'
            $kind = if ($has) { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
            $userPath = if ($has) {
                [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
            else { '' }
            if (@($userPath -split ';') -notcontains $installDir) {
                $key.SetValue('Path', $(if ($userPath) { "$installDir;$userPath" } else { $installDir }), $kind)
                $added = $true
            }
        }
        finally { if ($key) { $key.Close() } }
        if ($added) {
            # A raw registry write sends no broadcast, so running shells are told to reread it here.
            try {
                if (-not ('Penv.Native' -as [type])) {
                    Add-Type -Namespace Penv -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
                }
                $answered = [UIntPtr]::Zero
                $null = [Penv.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 1000, [ref]$answered)
            }
            catch { }
            Write-Host "added $installDir to your user PATH"
        }
        $env:Path = "$installDir;$env:Path"
        # Run from a file, this process is about to end and take its PATH with it.
        if ($PSCommandPath -and -not $onPath) { Write-Host 'open a new terminal, then: penv init' }
        else { Write-Host 'run: penv init' }
    }
    elseif ($onPath) {
        Write-Host 'run: penv init'
    }
    else {
        Write-Host ''
        Write-Host "this session:      `$env:Path = `"$installDir;`$env:Path`""
        Write-Host "every session:     rerun this installer with -AddToPath, or `$env:PENV_ADD_TO_PATH = '1' when you pipe it in"
        Write-Host "or run it now:     & `"$binary`" init"
    }
} @args
