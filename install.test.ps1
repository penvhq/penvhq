# install.ps1 against a fake release served from a local directory, with no Pester:
# powershell -NoProfile -ExecutionPolicy Bypass -File install.test.ps1

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$shipped = Join-Path $here 'install.ps1'
$keyLine = '(?m)^(\s*\$publicKeys = )''[^'']*'''
$tag = 'v9.9.9'
# The machine's own value, the way the installer reads it, so an x64 shell on ARM64 agrees with it.
$machine = [Microsoft.Win32.Registry]::GetValue(
    'HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'PROCESSOR_ARCHITECTURE', $env:PROCESSOR_ARCHITECTURE)
$triple = if ($machine -eq 'ARM64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$asset = "penv-$tag-$triple.exe"
$sums = "penv-$tag-$triple.sha256"

$me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if ($me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'skip: install.ps1 refuses an elevated shell, so run this test in a normal one'
    exit 0
}

# Probed rather than looked up, since Windows puts a store stub on PATH under both names.
$python = $null
$ErrorActionPreference = 'Continue'
foreach ($candidate in @('python3', 'python', 'py')) {
    if (-not (Get-Command $candidate -ErrorAction SilentlyContinue)) { continue }
    & $candidate -c 'import sys; sys.exit(0 if sys.version_info >= (3, 7) else 1)' 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { $python = $candidate; break }
}
$ErrorActionPreference = 'Stop'
if (-not $python) {
    Write-Host 'skip: python 3.7 or newer serves the fake release, and there is none on PATH'
    exit 0
}

# Resolved once, since the case with no OpenSSL runs it under a PATH without one.
$shell = (Get-Command powershell.exe).Source

$failures = 0
function Check($name, $actual, $expected) {
    if ($actual -eq $expected) { Write-Host "ok   $name" }
    else {
        Write-Host "FAIL $name"
        Write-Host "     wanted: $expected"
        Write-Host "     got:    $actual"
        $script:failures++
    }
}
function Mentions($name, $haystack, $needle) {
    if ($haystack -like "*$needle*") { Write-Host "ok   $name" }
    else {
        Write-Host "FAIL $name"
        Write-Host "     wanted a mention of: $needle"
        Write-Host "     got:                 $haystack"
        $script:failures++
    }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("penv-install-test-" + [Guid]::NewGuid().ToString('N'))
$server = $null
try {
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    # The installer with its key list set, the way install.test.sh edits public_keys.
    function WithKeys($keys, $path) {
        $text = [Regex]::Replace((Get-Content $shipped -Raw), $keyLine, { param($m) $m.Groups[1].Value + "'$keys'" })
        Set-Content -Path $path -Value $text -Encoding Ascii -NoNewline
        $path
    }
    # The unsigned cases run the installer as it ships with no release key pasted in.
    $installer = WithKeys '' (Join-Path $work 'install-keyless.ps1')
    Check 'the test emptied the key list in its copy of the installer' `
    ((Get-Content $installer -Raw) -match "(?m)^\s*\`$publicKeys = ''\s*`$") $true

    # Three releases laid out the way penv.cloud redirects to them: one whole, one whose
    # digest lies, one whose sums file only covers the archive.
    function Assets($name) { Join-Path $work "serve\$name\releases\download\$tag" }
    foreach ($name in @('good', 'tampered', 'archive-only')) {
        $dir = Assets $name
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        Set-Content -Path (Join-Path $dir $asset) -Value 'not a real binary' -Encoding Ascii -NoNewline
        Set-Content -Path (Join-Path $dir "$asset.zip") -Value 'not a real archive' -Encoding Ascii -NoNewline
        Set-Content -Path (Join-Path $work "serve\$name\releases\latest") -Encoding Ascii -NoNewline `
            -Value "{""tag_name"": ""$tag"", ""assets"": [{""name"": ""$asset""}, {""name"": ""$sums""}]}"
    }
    function Line($dir, $name) {
        $hash = (Get-FileHash -Path (Join-Path $dir $name) -Algorithm SHA256).Hash.ToLower()
        "$hash  $name"
    }
    $good = Assets 'good'
    Set-Content -Path (Join-Path $good $sums) -Encoding Ascii -Value @((Line $good "$asset.zip"), (Line $good $asset))
    $tampered = Assets 'tampered'
    $wrong = 'e' * 64
    Set-Content -Path (Join-Path $tampered $sums) -Encoding Ascii -Value @((Line $tampered "$asset.zip"), "$wrong  $asset")
    $only = Assets 'archive-only'
    Set-Content -Path (Join-Path $only $sums) -Encoding Ascii -Value @(Line $only "$asset.zip")

    # Files rather than python -c, because PowerShell 5.1 mangles quotes on a native command line.
    $portScript = Join-Path $work 'port.py'
    Set-Content -Path $portScript -Encoding Ascii -Value @(
        'import socket',
        's = socket.socket()',
        "s.bind(('127.0.0.1', 0))",
        'print(s.getsockname()[1])',
        's.close()')
    $port = & $python $portScript

    $server = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList @('-m', 'http.server', $port, '--bind', '127.0.0.1', '--directory', (Join-Path $work 'serve'))

    $waitScript = Join-Path $work 'wait.py'
    Set-Content -Path $waitScript -Encoding Ascii -Value @(
        'import sys, time, urllib.request',
        'for _ in range(100):',
        '    try:',
        "        urllib.request.urlopen('http://127.0.0.1:$port/good/releases/latest').read()",
        '        sys.exit(0)',
        '    except Exception:',
        '        time.sleep(0.1)',
        'sys.exit(1)')
    & $python $waitScript
    if ($LASTEXITCODE -ne 0) { throw "the fake release never came up on 127.0.0.1:$port" }

    # 'Continue' locally, so a refusal on the child's stderr is output to read rather than a thrown error.
    function Capture([string[]] $arguments) {
        $ErrorActionPreference = 'Continue'
        $script:output = (& $shell $arguments 2>&1 | Out-String)
        $script:code = $LASTEXITCODE
    }
    function Install($release, $into, $extra) { InstallWith $installer $release $into $extra }
    function InstallWith($script, $release, $into, $extra) {
        Capture (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script,
                '-Version', $tag, '-ReleaseBase', "http://127.0.0.1:$port/$release", '-InstallDir', $into) + $extra)
    }

    $into = Join-Path $work 'bin'
    Install 'good' $into @()
    Check 'a whole release installs' $code 0
    Check 'the binary lands where -InstallDir says' `
    (Get-Content (Join-Path $into 'penv.exe') -Raw -ErrorAction SilentlyContinue) 'not a real binary'
    Mentions 'the install location is printed' $output (Join-Path $into 'penv.exe')
    Mentions 'an installer with no release key says the signature went unchecked' $output `
        'signature not checked: this installer carries no release key'

    # No -Version, so the tag comes from the release the base answers with.
    $into = Join-Path $work 'unpinned-bin'
    Capture @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer,
        '-ReleaseBase', "http://127.0.0.1:$port/good", '-InstallDir', $into)
    Check 'an unpinned install reads the tag off the latest release' $code 0
    Mentions 'the tag it resolved is printed' $output "penv $tag"

    $into = Join-Path $work 'tampered-bin'
    Install 'tampered' $into @()
    Check 'a digest that does not match refuses' ($code -ne 0) $true
    Mentions 'the refusal names the file' $output 'is not the file'
    Check 'nothing is installed after a mismatch' (Test-Path (Join-Path $into 'penv.exe')) $false

    $into = Join-Path $work 'archive-bin'
    Install 'archive-only' $into @()
    Check 'a sums file covering only the archive refuses' ($code -ne 0) $true
    Mentions 'the refusal names the missing digest' $output 'lists no sha256 digest'

    $into = Join-Path $work 'bare-bin'
    Capture @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer,
        '-Version', $tag.TrimStart('v'), '-ReleaseBase', "http://127.0.0.1:$port/good", '-InstallDir', $into)
    Check 'a tag with no v installs the same release' $code 0
    Check 'the binary lands from the normalised tag' (Test-Path (Join-Path $into 'penv.exe')) $true

    $into = Join-Path $work 'slash-bin'
    Capture @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer,
        '-Version', "$tag/../etc", '-ReleaseBase', "http://127.0.0.1:$port/good", '-InstallDir', $into)
    Check 'a tag holding a slash refuses' ($code -ne 0) $true
    Mentions 'the refusal says what a tag looks like' $output 'is not a tag such as v1.2.3'

    $into = Join-Path $work 'flag-bin'
    Install 'good' $into @('-Nonsense')
    Check 'an unknown flag refuses' ($code -ne 0) $true
    Mentions 'the refusal names the flag' $output 'not a flag this installer takes'

    # Piped in, there are no flags to pass, so the same four settings arrive as environment variables.
    $into = Join-Path $work 'piped-bin'
    $piped = @"
`$env:PENV_VERSION = '$tag'
`$env:PENV_RELEASE_BASE = 'http://127.0.0.1:$port/good'
`$env:PENV_INSTALL_DIR = '$into'
Invoke-Expression (Get-Content '$installer' -Raw)
"@
    Capture @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $piped)
    Check 'the script installs when it is piped through iex' `
    (Get-Content (Join-Path $into 'penv.exe') -Raw -ErrorAction SilentlyContinue) 'not a real binary'

    # The installer's own rule: OpenSSL 1.1.1 or newer, and not LibreSSL, so a host where it
    # would not verify anyway skips these.
    $openssl = Get-Command openssl -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    function Openssl {
        $ErrorActionPreference = 'Continue'
        & $openssl.Source @args 2>$null | Out-Null
        $LASTEXITCODE -eq 0
    }
    $verifies = $false
    if ($openssl) {
        $ErrorActionPreference = 'Continue'
        $said = (& $openssl.Source version 2>$null | Out-String).Trim()
        $ErrorActionPreference = 'Stop'
        if ($said -match '^OpenSSL (\d+)\.(\d+)\.(\d+)') {
            $major = [int]$Matches[1]; $minor = [int]$Matches[2]; $patch = [int]$Matches[3]
            $verifies = $major -gt 1 -or ($major -eq 1 -and ($minor -gt 1 -or ($minor -eq 1 -and $patch -ge 1)))
        }
    }
    $signerKey = Join-Path $work 'signer.pem'
    if (-not $verifies -or -not (Openssl genpkey -algorithm ed25519 -out $signerKey)) {
        Write-Host 'skip: openssl 1.1.1 or newer signs the fake releases, and there is none on PATH'
    }
    else {
        $otherKey = Join-Path $work 'other.pem'
        $null = Openssl genpkey -algorithm ed25519 -out $otherKey
        # The raw 32 key bytes, which is the shape the installer and the binary list.
        $der = Join-Path $work 'signer.der'
        $null = Openssl pkey -in $signerKey -pubout -outform DER -out $der
        $derBytes = [IO.File]::ReadAllBytes($der)
        $public = [Convert]::ToBase64String($derBytes, $derBytes.Length - 32, 32)

        function SignSums($key, $dir) {
            $bin = Join-Path $work 'signature.bin'
            $null = Openssl pkeyutl -sign -inkey $key -rawin -in (Join-Path $dir $sums) -out $bin
            # One base64 line and a newline, which is what penv-release writes.
            [IO.File]::WriteAllText((Join-Path $dir "$sums.sig"), [Convert]::ToBase64String([IO.File]::ReadAllBytes($bin)) + "`n")
        }
        foreach ($name in @('signed', 'bad-signature')) {
            $dir = Assets $name
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            foreach ($file in @($asset, "$asset.zip", $sums)) { Copy-Item -Path (Join-Path $good $file) -Destination $dir }
            Copy-Item -Path (Join-Path $work 'serve\good\releases\latest') -Destination (Join-Path $work "serve\$name\releases\latest")
        }
        SignSums $signerKey (Assets 'signed')
        SignSums $otherKey (Assets 'bad-signature')

        # The installer as it ships once the owner has pasted a release key into it.
        $signer = WithKeys $public (Join-Path $work 'install-signed.ps1')
        Check 'the test embedded a key in its copy of the installer' `
        ((Get-Content $signer -Raw).Contains("`$publicKeys = '$public'")) $true

        $into = Join-Path $work 'signed-bin'
        InstallWith $signer 'signed' $into @()
        Check 'a signed release installs' $code 0
        Check 'the signed binary lands' (Test-Path (Join-Path $into 'penv.exe')) $true
        Check 'a signed release is verified rather than waved through' ($output -like '*signature not checked*') $false

        # The same key as somebody might paste it, without the = base64 ends on.
        $unpadded = $public.TrimEnd('=')
        Check 'the release key ends on the padding this case drops' $unpadded.Length 43
        $bare = WithKeys $unpadded (Join-Path $work 'install-unpadded.ps1')
        $into = Join-Path $work 'unpadded-bin'
        InstallWith $bare 'signed' $into @()
        Check 'a key pasted without its padding still verifies' $code 0
        Check 'the binary lands under an unpadded key' (Test-Path (Join-Path $into 'penv.exe')) $true
        Check 'an unpadded key verifies rather than being waved through' ($output -like '*signature not checked*') $false

        $into = Join-Path $work 'badsig-bin'
        InstallWith $signer 'bad-signature' $into @()
        Check 'a signature from another key refuses' ($code -ne 0) $true
        Mentions 'the refusal names the key list' $output 'is not signed by a penv release key'
        Check 'nothing is installed after a bad signature' (Test-Path (Join-Path $into 'penv.exe')) $false

        $into = Join-Path $work 'nosig-bin'
        InstallWith $signer 'good' $into @()
        Check 'a release carrying no signature refuses' ($code -ne 0) $true
        Mentions 'the refusal says the signature is missing' $output '.sig could not be downloaded'
        Check 'nothing is installed without a signature' (Test-Path (Join-Path $into 'penv.exe')) $false

        # With no OpenSSL on PATH the installer falls back to the digest and says so, as install.sh does.
        $drop = @(Get-Command openssl -CommandType Application -All | ForEach-Object { Split-Path -Parent $_.Source })
        $savedPath = $env:Path
        try {
            $env:Path = (@($env:Path -split [IO.Path]::PathSeparator | Where-Object { $_ -and $drop -notcontains $_ }) -join [IO.Path]::PathSeparator)
            $into = Join-Path $work 'no-openssl-bin'
            InstallWith $signer 'bad-signature' $into @()
        }
        finally { $env:Path = $savedPath }
        Check 'with no OpenSSL a release installs on its digest' $code 0
        Check 'the binary lands with no OpenSSL' (Test-Path (Join-Path $into 'penv.exe')) $true
        Mentions 'and says the signature went unchecked' $output `
            'signature not checked: OpenSSL 1.1.1 or newer verifies it, and this host has none'
    }
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures -eq 0) { Write-Host 'install.ps1: all checks passed' }
else {
    Write-Host "install.ps1: $failures failed"
    exit 1
}
