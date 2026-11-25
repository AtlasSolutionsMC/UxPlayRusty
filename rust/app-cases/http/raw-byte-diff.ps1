param(
    [string] $RustExecutable = 'rust/app/target/acceptance-release/release/uxplay.exe',
    [string] $ReferenceExecutable = 'baseline/original-build/uxplay.exe'
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$rustPath = (Resolve-Path (Join-Path $repoRoot $RustExecutable)).Path
$referencePath = (Resolve-Path (Join-Path $repoRoot $ReferenceExecutable)).Path
$msysRoot = Join-Path $env:LOCALAPPDATA 'Programs\MSYS2\ucrt64'

$cases = @(
    [pscustomobject]@{ Name = 'action-missing-current-video'; Method = 'POST'; Path = '/action'; Headers = [ordered]@{ 'Content-Type' = 'application/x-apple-binary-plist'; 'X-Apple-Session-ID' = 'session' }; Body = 'invalid' },
    [pscustomobject]@{ Name = 'hls-master-missing-current-video'; Method = 'GET'; Path = '/master.m3u8'; Headers = [ordered]@{}; Body = $null },
    [pscustomobject]@{ Name = 'hls-media-missing-current-video'; Method = 'GET'; Path = '/media.m3u8'; Headers = [ordered]@{}; Body = $null },
    [pscustomobject]@{ Name = 'play-invalid-body-with-session'; Method = 'POST'; Path = '/play'; Headers = [ordered]@{ 'X-Apple-Session-ID' = 'session' }; Body = $null },
    [pscustomobject]@{ Name = 'play-missing-session'; Method = 'POST'; Path = '/play'; Headers = [ordered]@{}; Body = $null },
    [pscustomobject]@{ Name = 'playback-info'; Method = 'GET'; Path = '/playback-info'; Headers = [ordered]@{ 'X-Apple-Session-ID' = 'session' }; Body = $null },
    [pscustomobject]@{ Name = 'server-info'; Method = 'GET'; Path = '/server-info'; Headers = [ordered]@{ 'X-Apple-Session-ID' = 'session' }; Body = $null },
    [pscustomobject]@{ Name = 'trap-001-get-rust-anything-missin'; Method = 'GET'; Path = '/rust-anything-missing'; Headers = [ordered]@{}; Body = $null }
)

function Get-FreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { return [int] $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Wait-ForServer([System.Diagnostics.Process] $Process, [int] $Port, [string] $StderrPath) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $deadline) {
        $Process.Refresh()
        if ($Process.HasExited) {
            $stderr = Get-Content -LiteralPath $StderrPath -Raw -ErrorAction SilentlyContinue
            throw "Server exited before listening (code $($Process.ExitCode)): $stderr"
        }
        $probe = [System.Net.Sockets.TcpClient]::new()
        try {
            $probe.Connect('127.0.0.1', $Port)
            return
        }
        catch {
            Start-Sleep -Milliseconds 100
        }
        finally {
            $probe.Dispose()
        }
    }
    throw "Server did not listen on 127.0.0.1:$Port within 20 seconds"
}

function New-RequestBytes([pscustomobject] $Case, [int] $Port) {
    $head = "$($Case.Method) $($Case.Path) HTTP/1.1`r`nHost: 127.0.0.1:$Port`r`nConnection: close`r`n"
    foreach ($header in $Case.Headers.GetEnumerator()) {
        $head += "$($header.Key): $($header.Value)`r`n"
    }
    $bodyBytes = [byte[]]@()
    if ($null -ne $Case.Body -or $Case.Method -in @('POST', 'PUT', 'PATCH')) {
        if ($null -ne $Case.Body) { $bodyBytes = [Text.Encoding]::UTF8.GetBytes([string] $Case.Body) }
        $head += "Content-Length: $($bodyBytes.Length)`r`n"
    }
    $head += "`r`n"
    $headBytes = [Text.Encoding]::ASCII.GetBytes($head)
    $request = [byte[]]::new($headBytes.Length + $bodyBytes.Length)
    [Buffer]::BlockCopy($headBytes, 0, $request, 0, $headBytes.Length)
    [Buffer]::BlockCopy($bodyBytes, 0, $request, $headBytes.Length, $bodyBytes.Length)
    return ,$request
}

function Receive-Response([byte[]] $Request, [int] $Port) {
    $client = [System.Net.Sockets.TcpClient]::new()
    $client.ReceiveTimeout = 10000
    $client.SendTimeout = 10000
    try {
        $client.Connect('127.0.0.1', $Port)
        $stream = $client.GetStream()
        $stream.Write($Request, 0, $Request.Length)
        $buffer = [byte[]]::new(8192)
        $response = [System.IO.MemoryStream]::new()
        $separatorFound = $false
        $headerEnd = -1
        while (-not $separatorFound) {
            $read = $stream.Read($buffer, 0, $buffer.Length)
            if ($read -eq 0) { break }
            $response.Write($buffer, 0, $read)
            $received = $response.ToArray()
            for ($index = 0; $index -le $received.Length - 4; $index++) {
                if ($received[$index] -eq 13 -and $received[$index + 1] -eq 10 -and
                    $received[$index + 2] -eq 13 -and $received[$index + 3] -eq 10) {
                    $headerEnd = $index
                    $separatorFound = $true
                    break
                }
            }
        }
        if (-not $separatorFound) { return ,$response.ToArray() }

        $header = [Text.Encoding]::ASCII.GetString($response.ToArray(), 0, $headerEnd)
        $contentLength = [regex]::Match($header, '(?im)^Content-Length:\s*(\d+)\s*$')
        if ($contentLength.Success) {
            $expectedLength = $headerEnd + 4 + [int] $contentLength.Groups[1].Value
            while ($response.Length -lt $expectedLength) {
                $read = $stream.Read($buffer, 0, $buffer.Length)
                if ($read -eq 0) { break }
                $response.Write($buffer, 0, $read)
            }
            return ,$response.ToArray()
        }

        try { $client.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send) }
        catch [System.Net.Sockets.SocketException] {}
        try {
            while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $response.Write($buffer, 0, $read)
            }
        }
        catch [System.IO.IOException] {
            if ($response.Length -eq 0) { throw }
        }
        return ,$response.ToArray()
    }
    finally {
        $client.Dispose()
    }
}

function Normalize-DateHeader([byte[]] $Response) {
    $separator = [byte[]](13, 10, 13, 10)
    $headerEnd = -1
    for ($index = 0; $index -le $Response.Length - $separator.Length; $index++) {
        if ($Response[$index] -eq 13 -and $Response[$index + 1] -eq 10 -and
            $Response[$index + 2] -eq 13 -and $Response[$index + 3] -eq 10) {
            $headerEnd = $index
            break
        }
    }
    if ($headerEnd -lt 0) { throw 'Response has no complete HTTP header block' }
    $header = [Text.Encoding]::ASCII.GetString($Response, 0, $headerEnd)
    $header = [regex]::Replace($header, '(?im)^Date:[^\r\n]*', 'Date: <normalized>')
    $headerBytes = [Text.Encoding]::ASCII.GetBytes($header)
    $normalized = [byte[]]::new($headerBytes.Length + $separator.Length + $Response.Length - $headerEnd - $separator.Length)
    [Buffer]::BlockCopy($headerBytes, 0, $normalized, 0, $headerBytes.Length)
    [Buffer]::BlockCopy($separator, 0, $normalized, $headerBytes.Length, $separator.Length)
    $bodyLength = $Response.Length - $headerEnd - $separator.Length
    if ($bodyLength -gt 0) {
        [Buffer]::BlockCopy($Response, $headerEnd + $separator.Length, $normalized, $headerBytes.Length + $separator.Length, $bodyLength)
    }
    return [Convert]::ToBase64String($normalized)
}

function Format-BytesHex([AllowNull()][AllowEmptyCollection()][byte[]] $Bytes) {
    if ($null -eq $Bytes -or $Bytes.Length -eq 0) { return '<empty>' }
    return [BitConverter]::ToString($Bytes).Replace('-', '')
}

function Capture-Build([string] $Executable, [bool] $RustServer, [pscustomobject] $Case) {
    $port = Get-FreePort
    $token = [guid]::NewGuid().ToString('N')
    $stdoutPath = Join-Path $env:TEMP "uxplay-http-$token.out"
    $stderrPath = Join-Path $env:TEMP "uxplay-http-$token.err"
    $savedPath = $env:PATH
    $savedPluginPath = [Environment]::GetEnvironmentVariable('GST_PLUGIN_PATH_1_0', 'Process')
    $savedScanner = [Environment]::GetEnvironmentVariable('GST_PLUGIN_SCANNER_1_0', 'Process')
    $savedLang = [Environment]::GetEnvironmentVariable('LANG', 'Process')
    $process = $null
    try {
        $env:PATH = "$msysRoot\bin;$env:PATH"
        $env:GST_PLUGIN_PATH_1_0 = "$msysRoot\lib\gstreamer-1.0"
        $env:GST_PLUGIN_SCANNER_1_0 = "$msysRoot\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
        $env:LANG = 'C.UTF-8'
        $process = Start-Process -FilePath $Executable `
            -ArgumentList @('-p', 'tcp', "64000,$port,64002", '-hls') `
            -WorkingDirectory $repoRoot -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
        Wait-ForServer $process $port $stderrPath
        $request = New-RequestBytes $Case $port
        try {
            $raw = Receive-Response $request $port
        }
        catch {
            $buildName = if ($RustServer) { 'Rust' } else { 'reference' }
            throw "$buildName case $($Case.Name) could not be read: $($_.Exception.Message)"
        }
        try {
            $normalized = Normalize-DateHeader $raw
            return [string] $normalized
        }
        catch {
            $buildName = if ($RustServer) { 'Rust' } else { 'reference' }
            $hex = Format-BytesHex $raw
            throw "$buildName case $($Case.Name) returned no complete response header; bytes: $hex"
        }
    }
    finally {
        if ($null -ne $process) {
            $process.Refresh()
            if (-not $process.HasExited) {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                $process.WaitForExit(5000)
            }
        }
        $env:PATH = $savedPath
        if ($null -eq $savedPluginPath) { Remove-Item Env:GST_PLUGIN_PATH_1_0 -ErrorAction SilentlyContinue }
        else { $env:GST_PLUGIN_PATH_1_0 = $savedPluginPath }
        if ($null -eq $savedScanner) { Remove-Item Env:GST_PLUGIN_SCANNER_1_0 -ErrorAction SilentlyContinue }
        else { $env:GST_PLUGIN_SCANNER_1_0 = $savedScanner }
        if ($null -eq $savedLang) { Remove-Item Env:LANG -ErrorAction SilentlyContinue }
        else { $env:LANG = $savedLang }
        Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
    }
}

$reference = [ordered]@{}
$rust = [ordered]@{}
$referencePathString = $referencePath
$rustPathString = $rustPath
foreach ($case in $cases) {
    $reference[$case.Name] = Capture-Build $referencePathString $false $case
    $rust[$case.Name] = Capture-Build $rustPathString $true $case
}
$failures = 0
foreach ($case in $cases) {
    $matches = $reference[$case.Name] -ceq $rust[$case.Name]
    if ($matches) {
        "PASS $($case.Name)"
    }
    else {
        $failures++
        "FAIL $($case.Name)"
        "  C:    $(Format-BytesHex ([Convert]::FromBase64String($reference[$case.Name])))"
        "  Rust: $(Format-BytesHex ([Convert]::FromBase64String($rust[$case.Name])))"
    }
}
"Raw HTTP byte comparison: $($cases.Count - $failures)/$($cases.Count); only the Date header was normalized."
if ($failures -gt 0) { exit 1 }
