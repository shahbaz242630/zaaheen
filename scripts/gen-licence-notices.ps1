# Writes crates/vault-tauri/THIRD-PARTY-NOTICES.txt, the open-source notices
# the installer puts in the install folder (LEGAL-DOCUMENTS-DESIGN.md,
# decision L7: "the public installer must not ship without it").
#
#   Part 1: every Rust library in the shipped programs, from cargo-about
#           (about.toml + scripts/licence-notices.hbs).
#   Part 2: what is not a Rust library: ONNX Runtime, the AI models and the
#           app's fonts, from the texts in crates/vault-tauri/licences/.
#
# The file is committed: tauri-build copies every bundle resource on each
# compile and fails if one is missing, so it must exist in the repository.
#
#   .\scripts\gen-licence-notices.ps1          # regenerate the file
#   .\scripts\gen-licence-notices.ps1 -Check   # exit 1 if it is out of date
#
# The release build runs -Check first, so an installer never ships notices
# that differ from the committed, reviewed file.
#
# Needs cargo-about 0.9.2: cargo install cargo-about --version 0.9.2 --locked --features cli
# (pure ASCII on purpose: Windows PowerShell 5.1 misreads UTF-8 without a BOM)

param([switch]$Check)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Out = Join-Path $Root 'crates\vault-tauri\THIRD-PARTY-NOTICES.txt'
$Texts = Join-Path $Root 'crates\vault-tauri\licences'
$AboutVersion = '0.9.2'

$have = (& cargo about --version 2>$null) -join ''
if ($have -notmatch [regex]::Escape($AboutVersion)) {
    Write-Error "cargo-about $AboutVersion is required (found: '$have'). Install: cargo install cargo-about --version $AboutVersion --locked --features cli"
}

# Part 2: one entry per component that is not a Rust library. Version and
# source are what the installer carries (tauri.conf.json resources) or
# downloads on first use; update them together.
$Components = @(
    @{ Name = 'ONNX Runtime 1.22.0 (onnxruntime.dll)'; Source = 'https://github.com/microsoft/onnxruntime'; Licence = 'MIT';
       Files = @('onnxruntime-LICENSE.txt', 'onnxruntime-ThirdPartyNotices.txt') },
    @{ Name = 'BAAI bge-small-en-v1.5 (search model, shipped)'; Source = 'https://huggingface.co/BAAI/bge-small-en-v1.5'; Licence = 'MIT';
       Files = @('bge-small-en-v1.5-LICENSE.txt') },
    @{ Name = 'Qwen3-Reranker-0.6B (ranking model, downloaded on first use)'; Source = 'https://huggingface.co/Qwen/Qwen3-Reranker-0.6B'; Licence = 'Apache-2.0';
       Files = @('qwen3-reranker-0.6b-LICENSE-Apache-2.0.txt') },
    @{ Name = 'Microsoft Phi-4-mini-instruct (tidying model, downloaded when tidying is turned on)'; Source = 'https://huggingface.co/microsoft/Phi-4-mini-instruct'; Licence = 'MIT';
       Files = @('phi-4-mini-instruct-LICENSE.txt', 'phi-4-mini-instruct-NOTICE.md') },
    @{ Name = 'Newsreader (font)'; Source = 'https://github.com/productiontype/Newsreader'; Licence = 'OFL-1.1';
       Files = @('font-newsreader-OFL.txt') },
    @{ Name = 'IBM Plex Sans and IBM Plex Mono (fonts)'; Source = 'https://github.com/IBM/plex'; Licence = 'OFL-1.1';
       Files = @('font-ibm-plex-OFL.txt') }
)

$tmp = [System.IO.Path]::GetTempFileName()
try {
    Push-Location $Root
    try {
        & cargo about generate --workspace --locked --fail -c about.toml -o $tmp scripts/licence-notices.hbs
        if ($LASTEXITCODE -ne 0) { Write-Error "cargo about generate failed (exit $LASTEXITCODE)" }
    } finally { Pop-Location }
    $part1 = [System.IO.File]::ReadAllText($tmp)
} finally { Remove-Item $tmp -ErrorAction SilentlyContinue }

$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine('ZAAHEEN THIRD-PARTY NOTICES')
[void]$sb.AppendLine('')
[void]$sb.AppendLine('Zaaheen includes open-source software, AI models and fonts made by others,')
[void]$sb.AppendLine('used under their own licences. Their notices follow. See also')
[void]$sb.AppendLine('https://zaaheen.com/licences/')
[void]$sb.AppendLine('')
[void]$sb.Append($part1)
[void]$sb.AppendLine('')
[void]$sb.AppendLine('PART 2. OTHER COMPONENTS: AI RUNTIME, AI MODELS AND FONTS')
[void]$sb.AppendLine('')
foreach ($c in $Components) {
    [void]$sb.AppendLine('------------------------------------------------------------------------------')
    [void]$sb.AppendLine($c.Name)
    [void]$sb.AppendLine("Source: $($c.Source)")
    [void]$sb.AppendLine("Licence: $($c.Licence)")
    [void]$sb.AppendLine('')
    foreach ($f in $c.Files) {
        $p = Join-Path $Texts $f
        if (-not (Test-Path $p)) { Write-Error "missing licence text: $p" }
        [void]$sb.AppendLine([System.IO.File]::ReadAllText($p).TrimEnd())
        [void]$sb.AppendLine('')
    }
}

# One line-ending style (LF), so the check compares content, not checkout settings.
$new = ($sb.ToString() -replace "`r`n", "`n").TrimEnd() + "`n"

if ($Check) {
    if (-not (Test-Path $Out)) { Write-Host "THIRD-PARTY-NOTICES.txt is missing: run scripts\gen-licence-notices.ps1 and commit it"; exit 1 }
    $old = ([System.IO.File]::ReadAllText($Out) -replace "`r`n", "`n")
    if ($old -ne $new) { Write-Host 'THIRD-PARTY-NOTICES.txt is out of date: run scripts\gen-licence-notices.ps1, review and commit it'; exit 1 }
    Write-Host 'THIRD-PARTY-NOTICES.txt is up to date.'
    exit 0
}

[System.IO.File]::WriteAllText($Out, $new, (New-Object System.Text.UTF8Encoding $false))
Write-Host "Wrote $Out"
