#Requires -Version 5.1
<#
.SYNOPSIS
    Installs the mcp-ollama-web-rust MCP server on a deployed Windows machine.

.DESCRIPTION
    Copies the executable into an install directory, creates .credentials from the
    committed template only when no credential file exists, and generates an MCP
    client entry (mcp-config.json) with absolute paths.

    Existing .credentials files are never overwritten.

.PARAMETER InstallDir
    Install directory. Defaults to %LOCALAPPDATA%\Programs\mcp-ollama-web-rust.

.PARAMETER HttpProxy
    Optional HTTP proxy URL. Omit when the machine is not behind a proxy.

.PARAMETER HttpsProxy
    Optional HTTPS proxy URL. Omit when the machine is not behind a proxy.

.PARAMETER NoProxy
    Optional NO_PROXY list, for example
    "localhost,127.0.0.1,*.company.com,.company.com".

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 `
      -InstallDir "C:\Tools\mcp-ollama-web-rust" `
      -HttpProxy "http://proxy_ip:proxy_port" `
      -HttpsProxy "http://proxy_ip:proxy_port" `
      -NoProxy "localhost,127.0.0.1,*.company.com,.company.com"
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\mcp-ollama-web-rust'),
    [string]$HttpProxy = '',
    [string]$HttpsProxy = '',
    [string]$NoProxy = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$required = @(
    'mcp-ollama-web-rust.exe',
    'credentials.example',
    'INSTALL.md',
    'LICENSE'
)
foreach ($name in $required) {
    $path = Join-Path $source $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Bundle is incomplete: '$path' is missing."
    }
}

$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$binary = Join-Path $InstallDir 'mcp-ollama-web-rust.exe'
$credentials = Join-Path $InstallDir '.credentials'
$configPath = Join-Path $InstallDir 'mcp-config.json'

Copy-Item -LiteralPath (Join-Path $source 'mcp-ollama-web-rust.exe') -Destination $binary -Force
Copy-Item -LiteralPath (Join-Path $source 'LICENSE') -Destination (Join-Path $InstallDir 'LICENSE') -Force
Copy-Item -LiteralPath (Join-Path $source 'INSTALL.md') -Destination (Join-Path $InstallDir 'INSTALL.md') -Force
Copy-Item -LiteralPath (Join-Path $source 'credentials.example') -Destination (Join-Path $InstallDir 'credentials.example') -Force

$createdCredentials = $false
if (-not (Test-Path -LiteralPath $credentials -PathType Leaf)) {
    Copy-Item -LiteralPath (Join-Path $source 'credentials.example') -Destination $credentials
    $createdCredentials = $true
}

$envVars = [ordered]@{
    OLLAMA_HOST = 'https://ollama.com'
    OLLAMA_CREDENTIALS_FILE = $credentials
}
if (-not [string]::IsNullOrWhiteSpace($HttpProxy)) {
    $envVars['HTTP_PROXY'] = $HttpProxy.Trim()
}
if (-not [string]::IsNullOrWhiteSpace($HttpsProxy)) {
    $envVars['HTTPS_PROXY'] = $HttpsProxy.Trim()
}
if (-not [string]::IsNullOrWhiteSpace($NoProxy)) {
    $envVars['NO_PROXY'] = $NoProxy.Trim()
}

$config = [ordered]@{
    mcpServers = [ordered]@{
        'ollama-web' = [ordered]@{
            command = $binary
            env = $envVars
        }
    }
}
$json = $config | ConvertTo-Json -Depth 8
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($configPath, $json, $utf8NoBom)

$version = & $binary --version
if ($LASTEXITCODE -ne 0) {
    throw "The installed server did not run: $binary"
}

Write-Host ''
Write-Host 'mcp-ollama-web-rust installed.'
Write-Host "  Install directory : $InstallDir"
Write-Host "  Version           : $version"
Write-Host "  MCP config        : $configPath"
if ($createdCredentials) {
    Write-Host "  Credentials       : $credentials (created from the template)"
    Write-Host ''
    Write-Host "IMPORTANT: edit $credentials and replace 'replace-with-your-key'"
    Write-Host 'with your Ollama API key before starting the MCP client.'
} else {
    Write-Host "  Credentials       : $credentials (existing file kept)"
}
Write-Host ''
Write-Host "Next: merge $configPath into your MCP client's server configuration,"
Write-Host 'then restart the client. See INSTALL.md for verification and troubleshooting.'
