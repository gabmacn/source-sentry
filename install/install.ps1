# Installs Source Sentry on Windows from a release archive.
#
#   irm https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.ps1 | iex
#
# With options, run it as a script block:
#
#   & ([scriptblock]::Create((irm https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.ps1))) -Version 0.1.0-rc.3
#
# It downloads the x86_64 Windows archive and the release's SHA256SUMS,
# refuses an archive whose checksum does not match, checks that the binary
# runs, and copies it into the install directory. It never changes PATH: if
# the directory is not on it, it says how to add it.
#
# Parameters:
#   -Version <version>  The release to install, such as 0.1.0. By default
#                       $env:SOURCE_SENTRY_VERSION, or else the latest release
#                       that is not a release candidate.
#   -Dir <directory>    Where to put source-sentry.exe. By default
#                       $env:SOURCE_SENTRY_INSTALL_DIR, or else
#                       %LOCALAPPDATA%\Programs\source-sentry.
#
# $env:SOURCE_SENTRY_RELEASES names the releases page to download from. By
# default https://git.gabmacn.com/gmn/source-sentry/releases;
# https://github.com/gabmacn/source-sentry/releases holds the same files.
#
# Written for Windows PowerShell 5.1 as well as PowerShell 7.

param(
	[string]$Version = $env:SOURCE_SENTRY_VERSION,
	[string]$Dir = $env:SOURCE_SENTRY_INSTALL_DIR
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar slows downloads in Windows PowerShell.
$ProgressPreference = 'SilentlyContinue'

function Stop-Install([string]$Message) {
	# Thrown rather than exiting, so that `irm | iex` does not close the
	# user's session.
	throw "install.ps1: $Message"
}

# Windows PowerShell 5.1 may not offer TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$releases = if ($env:SOURCE_SENTRY_RELEASES) { $env:SOURCE_SENTRY_RELEASES } else { 'https://git.gabmacn.com/gmn/source-sentry/releases' }
$releases = $releases.TrimEnd('/')
if (-not $Dir) {
	$Dir = Join-Path $env:LOCALAPPDATA 'Programs\source-sentry'
}

# Only x86_64 is built. Windows 11 on Arm runs it through emulation.
$machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
switch ($machine) {
	'AMD64' { }
	'ARM64' { Write-Host 'This is an Arm machine: installing the x86_64 build, which Windows 11 runs through emulation.' }
	default { Stop-Install "no release is built for the $machine architecture; build from source with cargo instead" }
}
$target = 'x86_64-pc-windows-msvc'

# The latest stable release, through the releases page's redirect. Both
# forges skip release candidates there.
if (-not $Version) {
	$latest = $null
	try {
		$response = Invoke-WebRequest -UseBasicParsing -Uri "$releases/latest"
		$latest = if ($response.BaseResponse.ResponseUri) {
			$response.BaseResponse.ResponseUri.AbsoluteUri
		} else {
			$response.BaseResponse.RequestMessage.RequestUri.AbsoluteUri
		}
	} catch {
		$latest = $null
	}
	if ($latest -match '/tag/v([^/?#]+)$') {
		$Version = $Matches[1]
	} else {
		Stop-Install "found no stable release at $releases; choose one with -Version, such as 0.1.0-rc.3"
	}
}
$Version = $Version.TrimStart('v')
if ($Version -notmatch '^[0-9A-Za-z.+-]+$') {
	Stop-Install "'$Version' is not a version"
}

$name = "source-sentry-$Version-$target"
$archive = "$name.zip"
$base = "$releases/download/v$Version"
$work = Join-Path ([IO.Path]::GetTempPath()) ("source-sentry-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
	Write-Host "Downloading Source Sentry $Version for $target."
	try {
		Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS')
	} catch {
		Stop-Install "could not download $base/SHA256SUMS; is $Version a released version?"
	}
	try {
		Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile (Join-Path $work $archive)
	} catch {
		Stop-Install "could not download $base/$archive"
	}

	# Lines are `<hash>  <name>`, or `<hash> *<name>` in binary mode.
	$expected = $null
	foreach ($line in Get-Content -LiteralPath (Join-Path $work 'SHA256SUMS')) {
		if ($line -match '^([0-9a-fA-F]{64}) [ *](.+)$' -and $Matches[2] -eq $archive) {
			$expected = $Matches[1].ToLowerInvariant()
		}
	}
	if (-not $expected) {
		Stop-Install "SHA256SUMS does not list $archive"
	}
	$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $work $archive)).Hash.ToLowerInvariant()
	if ($actual -ne $expected) {
		Stop-Install "the checksum of $archive does not match SHA256SUMS; nothing was installed"
	}
	Write-Host 'Checksum verified.'

	Expand-Archive -LiteralPath (Join-Path $work $archive) -DestinationPath $work
	$binary = Join-Path $work "$name\source-sentry.exe"
	if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
		Stop-Install "$archive does not contain $name\source-sentry.exe"
	}
	# Windows PowerShell turns a native program's standard error into
	# terminating errors under 'Stop'.
	$ErrorActionPreference = 'Continue'
	$reported = (& $binary --version 2>&1 | Out-String).Trim()
	$code = $LASTEXITCODE
	$ErrorActionPreference = 'Stop'
	if ($code -ne 0) {
		Stop-Install "the binary does not run on this machine: $reported"
	}
	if ($reported -ne "source-sentry $Version") {
		Stop-Install "the binary reports '$reported', not 'source-sentry $Version'"
	}

	New-Item -ItemType Directory -Force -Path $Dir | Out-Null
	$destination = Join-Path $Dir 'source-sentry.exe'
	# Copied beside its destination first, so that a failed copy never leaves
	# a partial binary in place.
	$temporary = Join-Path $Dir '.source-sentry.exe.tmp'
	Copy-Item -LiteralPath $binary -Destination $temporary -Force
	try {
		Move-Item -LiteralPath $temporary -Destination $destination -Force
	} catch {
		Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
		Stop-Install "could not replace $destination; is Source Sentry still running?"
	}
	Write-Host "Installed $reported to $destination."
} finally {
	Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

$full = [IO.Path]::GetFullPath($Dir).TrimEnd('\')
$onPath = @(($env:Path -split ';') | Where-Object { $_ } | ForEach-Object {
	try { [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($_)).TrimEnd('\') } catch { $_ }
}) -contains $full
if (-not $onPath) {
	Write-Host ''
	Write-Host 'The install directory is not on your PATH:'
	Write-Host "  $full"
	Write-Host 'To add it, open the environment variables dialog with'
	Write-Host '  rundll32 sysdm.cpl,EditEnvironmentVariables'
	Write-Host 'select Path under "User variables", choose Edit, then New, and paste the'
	Write-Host 'directory. Terminals opened afterwards will find source-sentry.'
}
Write-Host ''
Write-Host 'Shell completions: source-sentry completions --help'
