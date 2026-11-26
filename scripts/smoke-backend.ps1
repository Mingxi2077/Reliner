#requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory, Position = 0)]
    [string]$ZipPath,

    [Parameter(Position = 1)]
    [string]$PreviousZipPath
)

$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$ioTimeout = [TimeSpan]::FromSeconds(10)

function Assert-Smoke([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Invoke-Backend([string]$Package, [string[]]$Arguments, [scriptblock]$Exchange) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $Package 'reliner.exe'
    $start.WorkingDirectory = $Package
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardInputEncoding = $utf8
    $start.StandardOutputEncoding = $utf8
    $start.StandardErrorEncoding = $utf8
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }

    $command = "$Package/reliner.exe $($Arguments -join ' ')"
    $process = [Diagnostics.Process]::Start($start)
    $stdin = $process.StandardInput
    $stdout = $process.StandardOutput
    $stderr = $process.StandardError
    try {
        $stderrTask = $stderr.ReadToEndAsync()
        if ($Exchange) { & $Exchange $stdin $stdout }
        $stdoutTask = $stdout.ReadToEndAsync()
        $stdin.Close()

        Assert-Smoke ($process.WaitForExit([int]$ioTimeout.TotalMilliseconds)) "Process did not exit: $command"
        $output = $stdoutTask.WaitAsync($ioTimeout).GetAwaiter().GetResult()
        $errors = $stderrTask.WaitAsync($ioTimeout).GetAwaiter().GetResult()
        Assert-Smoke ($process.ExitCode -eq 0) "Exit code $($process.ExitCode): $command`n$errors$output"
        if ($Exchange) {
            Assert-Smoke ([string]::IsNullOrEmpty($output)) "Unexpected trailing JSONL output: $output"
        } else {
            return $output
        }
    } finally {
        try {
            if (-not $process.HasExited) {
                $process.Kill($true)
                Assert-Smoke ($process.WaitForExit(5000)) "Could not stop: $command"
            }
        } finally {
            # Do not flush a pending write if the child stopped reading stdin.
            $stdin.BaseStream.Dispose()
            $stdout.Dispose()
            $stderr.Dispose()
            $process.Dispose()
        }
    }
}

function Invoke-Workspace($Client, [hashtable]$Command) {
    $Client.RequestId++
    $id = [string]$Client.RequestId
    $type = $Command.type
    $line = @{
        protocol_version = 1
        request_id = $id
        session_id = $Client.SessionId
        command = $Command
    } | ConvertTo-Json -Depth 32 -Compress

    try {
        $null = $Client.Input.WriteLineAsync($line).WaitAsync($ioTimeout).GetAwaiter().GetResult()
        $null = $Client.Input.FlushAsync().WaitAsync($ioTimeout).GetAwaiter().GetResult()
        $raw = $Client.Output.ReadLineAsync().WaitAsync($ioTimeout).GetAwaiter().GetResult()
    } catch {
        throw "JSONL ${type} (request $id): $($_.Exception.GetBaseException().Message)"
    }
    Assert-Smoke ($null -ne $raw) "No JSONL response to $type (request $id)."
    $response = ConvertFrom-Json -AsHashtable -InputObject $raw
    Assert-Smoke ($response.protocol_version -eq 1 -and $response.request_id -is [string] -and
        $response.request_id -ceq $id) "Invalid JSONL response to ${type}: $raw"
    Assert-Smoke ($response.status -ceq 'ok') "JSONL ${type} failed: $raw"
    if ($type -ceq 'hello') {
        Assert-Smoke ($response.session_id -is [string] -and $response.session_id -match '^[0-9]+$' -and
            $response.data.session_id -ceq $response.session_id -and
            $response.data.protocol_version -eq 1) "Invalid JSONL handshake: $raw"
        $Client.SessionId = $response.session_id
    } else {
        Assert-Smoke ($response.session_id -ceq $Client.SessionId) "Unexpected session in response to ${type}: $raw"
    }
    return $response.data
}

$zip = (Resolve-Path -LiteralPath $ZipPath).ProviderPath
$previousZip = if ($PreviousZipPath) { (Resolve-Path -LiteralPath $PreviousZipPath).ProviderPath }
$scratch = [IO.Directory]::CreateTempSubdirectory('reliner-smoke-').FullName
try {
    $current = Join-Path $scratch 'current'
    [IO.Compression.ZipFile]::ExtractToDirectory($zip, $current)
    $gns3 = Join-Path $current 'examples/import-gns3.gns3'
    $models = Join-Path $current 'examples/import-gns3-models.json'
    $project = [IO.File]::ReadAllText($gns3, $utf8)
    $options = ConvertFrom-Json -AsHashtable -InputObject ([IO.File]::ReadAllText($models, $utf8))
    $importPath = Join-Path $scratch 'imported.json'
    # A non-ASCII destination must survive both the JSONL pipe and the filesystem.
    $archivePath = Join-Path $scratch "report $([char]0x6D4B).json"

    $null = Invoke-Backend $current @('import', 'gns3', $gns3, $models, $importPath)
    $native = ConvertFrom-Json -AsHashtable -InputObject ([IO.File]::ReadAllText($importPath, $utf8))
    Assert-Smoke ($native.import_source.format -ceq 'gns3' -and $native.id -ceq $options.id -and
        $native.devices.Count -eq 2 -and $native.links.Count -eq 1) 'CLI GNS3 import did not preserve the sample topology.'

    $live = Invoke-Backend $current @('workspace', '--jsonl') {
        param($stdin, $stdout)
        $client = @{ Input = $stdin; Output = $stdout; RequestId = 0; SessionId = $null }
        $hello = Invoke-Workspace $client @{ type = 'hello' }
        Assert-Smoke (-not [string]::IsNullOrWhiteSpace($hello.crate_version)) 'Handshake omitted the backend version.'
        if ([IO.Path]::GetFileName($zip) -match '^reliner-backend-(\d+\.\d+\.\d+)-x86_64-pc-windows-(?:msvc|gnu)\.zip$') {
            Assert-Smoke ($hello.crate_version -ceq $Matches[1]) "ZIP version $($Matches[1]) does not match backend $($hello.crate_version)."
        }
        if ([version]$hello.crate_version -ge [version]'0.4.0') {
            $expect = [IO.File]::ReadAllText((Join-Path $current 'examples/lldp-expected.json'), $utf8) | ConvertFrom-Json -AsHashtable
            $expect.port.value_hex = '45746831'
            $matched = Invoke-Workspace $client @{type='lldp.check';path=(Join-Path $current 'examples/lldp-peer.pcap');expectation=$expect}
            Assert-Smoke ($matched.status -ceq 'match' -and $matched.local_port -ceq $expect.local_port) 'JSONL LLDP corrected expectation did not match.'
        }

        $doc = Invoke-Workspace $client @{ type = 'import.gns3.open'; project = $project; options = $options }
        Assert-Smoke ($doc.topology_id -ceq $native.id -and $doc.native_revision -ceq $options.revision -and
            $doc.dirty -eq $true -and $null -eq $doc.path) 'JSONL GNS3 open did not create the expected unsaved document.'
        $submitted = Invoke-Workspace $client @{
            type = 'run.submit'; document_id = $doc.id; expected_revision = $doc.revision; mode = 'automatic'
        }
        $runId = $submitted.run.id
        Assert-Smoke ($runId -is [string] -and $runId -match '^[0-9]+$') 'Run submit did not return a decimal run ID.'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        do {
            $result = Invoke-Workspace $client @{ type = 'run.summary'; run_id = $runId }
            $state = $result.summary.status
            if ($state -ceq 'finished') { break }
            Assert-Smoke ($state -cin @('queued', 'compiling', 'ready', 'running')) (
                "Run $runId stopped unexpectedly: " + ($result.summary | ConvertTo-Json -Depth 12 -Compress))
            Assert-Smoke ($timer.Elapsed.TotalSeconds -lt 15) "Run $runId did not finish within 15 seconds (last status: $state)."
            Start-Sleep -Milliseconds 50
        } while ($true)
        Assert-Smoke ($result.origin.document_id -ceq $doc.id -and $result.origin.run_id -ceq $runId -and
            $result.summary.probe_counts.undetermined -eq 1) 'Run summary does not match the imported sample.'

        $probes = Invoke-Workspace $client @{ type = 'run.probes'; run_id = $runId; offset = 0; count = 1 }
        Assert-Smoke ($probes.entries.Count -eq 1 -and $null -eq $probes.next_offset) 'Expected one probe and no continuation.'
        $probe = $probes.entries[0]
        Assert-Smoke (
            $probe.id -ceq 'echo' -and $probe.outcome.status -ceq 'undetermined' -and
            $probe.outcome.modeled.status -ceq 'reachable') 'GNS3 sample should model a reachable echo but leave real reachability undetermined.'
        $first = Invoke-Workspace $client @{ type = 'run.evidence'; run_id = $runId; offset = 0; count = 1 }
        Assert-Smoke ($first.entries.Count -eq 1 -and $first.next_offset -eq 1) 'The first evidence page did not provide a continuation.'
        $second = Invoke-Workspace $client @{ type = 'run.evidence'; run_id = $runId; offset = $first.next_offset; count = 1 }
        Assert-Smoke ($second.entries.Count -eq 1 -and $second.entries[0].id -cne $first.entries[0].id) 'The next evidence page did not advance.'

        $closed = Invoke-Workspace $client @{
            type = 'document.close'; document_id = $doc.id; expected_revision = $doc.revision; discard_unsaved = $true
        }
        Assert-Smoke ($closed.closed -eq $true -and $closed.document_id -ceq $doc.id) 'The imported document was not closed.'
        $saved = Invoke-Workspace $client @{ type = 'run.archive_to'; run_id = $runId; path = $archivePath }
        Assert-Smoke ($saved.path -ceq $archivePath -and $saved.origin.run_id -ceq $runId) 'Archive response did not preserve the UTF-8 path or run ID.'
        $reopenedDoc = Invoke-Workspace $client @{ type = 'document.open'; path = $importPath }
        Assert-Smoke ($reopenedDoc.id -cne $doc.id -and $reopenedDoc.topology_id -ceq $doc.topology_id -and
            [IO.Path]::GetFileName($reopenedDoc.path) -ceq [IO.Path]::GetFileName($importPath) -and
            $reopenedDoc.dirty -eq $false) 'Could not reopen the CLI-imported experiment.'
        return @{ Version = $hello.crate_version; RunId = $runId; Revision = $doc.revision; Probe = $probe }
    }

    if ([version]$live.Version -ge [version]'0.4.0') {
        $route = Invoke-Backend $current @('real-stack', 'compare-frr', 'examples/stack-static-lab.json', 'examples/frr-route-observation.json') | ConvertFrom-Json -AsHashtable
        Assert-Smoke ($route.status -ceq 'match' -and $route.installed.kernel.protocol -eq 196 -and $null -eq $route.withdrawn.kernel) 'Packaged FRR route observation no longer matches the static topology.'
        $lldp = Invoke-Backend $current @('lldp', 'check', 'examples/lldp-peer.pcap', 'examples/lldp-expected.json', '--json') | ConvertFrom-Json -AsHashtable
        Assert-Smoke ($lldp.status -ceq 'wrong_peer' -and $lldp.neighbors.Count -eq 1 -and $lldp.neighbors[0].port.value_hex -ceq '45746831') 'Packaged LLDP observation did not identify the advertised port.'
        $importedSwitch = Join-Path $scratch 'switch.json'
        $null = Invoke-Backend $current @('import', 'containerlab', 'examples/import-containerlab.yml', 'examples/import-ios-models.json', $importedSwitch, '--ios-device', 'r1', 'examples/ios-switch.cfg')
        $switchRun = Invoke-Backend $current @('run', $importedSwitch, '--json') | ConvertFrom-Json -AsHashtable
        $outcome = $switchRun.probes[0].outcome
        Assert-Smoke ($outcome.status -ceq 'undetermined' -and $outcome.modeled.status -ceq 'timeout') 'Whole-switch import lost coverage or failed to reproduce the VLAN fault.'
        $acl = Invoke-Backend $current @('run', 'examples/acl-lab.json', '--json') | ConvertFrom-Json -AsHashtable
        $blocked = @($acl.evidence | Where-Object id -CEQ $acl.probes[0].outcome.cause)
        Assert-Smoke ($acl.report_version -eq 9 -and $blocked.Count -eq 1 -and $blocked[0].fact.kind -ceq 'ipv4_acl_decision' -and $blocked[0].fact.matched_rule.rule_number -eq 1) 'ACL denial missing its matching rule.'
    }
    $reopened = Invoke-Backend $current @('report', $archivePath, '--json') | ConvertFrom-Json -AsHashtable
    $archivedProbe = $reopened.report.probes | Select-Object -First 1
    Assert-Smoke ($reopened.report.topology_id -ceq $native.id -and $reopened.source.run_id -ceq $live.RunId -and
        $reopened.source.document_revision -ceq $live.Revision -and $reopened.report.probes.Count -eq 1 -and
        $archivedProbe.id -ceq $live.Probe.id -and $archivedProbe.outcome.status -ceq $live.Probe.outcome.status -and
        $archivedProbe.outcome.modeled.status -ceq $live.Probe.outcome.modeled.status -and
        $archivedProbe.outcome.modeled.at_ns -ceq $live.Probe.outcome.modeled.at_ns) 'Reopened archive did not preserve the GNS3 run and modeled result.'

    if ($previousZip) {
        $previous = Join-Path $scratch 'previous'
        [IO.Compression.ZipFile]::ExtractToDirectory($previousZip, $previous)
        $oldLab = Join-Path $previous 'examples/ipv4-lab.json'
        $oldArchivePath = Join-Path $scratch 'previous-report.json'
        $null = Invoke-Backend $previous @('archive', $oldLab, $oldArchivePath)
        $old = ConvertFrom-Json -AsHashtable -InputObject ([IO.File]::ReadAllText($oldArchivePath, $utf8))
        $new = Invoke-Backend $current @('report', $oldArchivePath, '--json') | ConvertFrom-Json -AsHashtable
        Assert-Smoke ($old.report.topology_id -ceq 'ipv4-lab' -and $new.report.topology_id -ceq $old.report.topology_id -and
            $new.report.revision -ceq $old.report.revision -and $old.report.probes.Count -eq 2 -and
            $new.report.probes.Count -eq $old.report.probes.Count) 'The previous native archive did not reopen with the expected topology and probes.'
        for ($i = 0; $i -lt $old.report.probes.Count; $i++) {
            $before = $old.report.probes[$i]
            $after = $new.report.probes[$i]
            Assert-Smoke ($before.outcome.status -ceq 'reachable' -and $after.id -ceq $before.id -and
                $after.outcome.status -ceq $before.outcome.status -and $after.outcome.at_ns -ceq $before.outcome.at_ns) (
                "Reopening the previous archive changed probe '$($before.id)'.")
        }
    }
} finally {
    # Remove only entries under the directory allocated by this invocation, deepest first.
    Get-ChildItem -LiteralPath $scratch -Force -Recurse |
        Sort-Object { $_.FullName.Length } -Descending |
        ForEach-Object {
            if ($_.PSIsContainer) { [IO.Directory]::Delete($_.FullName) }
            else { Remove-Item -LiteralPath $_.FullName -Force }
        }
    [IO.Directory]::Delete($scratch)
}

Write-Output "Backend $($live.Version): JSONL, GNS3 CLI import, and archive reopen passed."
if ($previousZip) { Write-Output 'Previous native archive compatibility passed.' }
