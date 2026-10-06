# setup-dev-env.ps1 — Windows equivalent of scripts/setup-dev-env.sh
#
# Downloads bge-small-en-v1.5 model.onnx + tokenizer.json + ONNX Runtime
# Windows DLL into crates/vault-embedding/test-fixtures/bge-small-en-v1.5/.
# Verifies SHA-256 of model.onnx against the canonical hash in
# crates/vault-embedding/src/integrity.rs.
#
# Idempotent: skips downloads if files exist with the correct hash.
# Re-run safely.
#
# ----------------------------------------------------------------------
# ORT_VERSION <-> ort crate version coupling - READ BEFORE BUMPING EITHER
# ----------------------------------------------------------------------
# The `ort` Rust crate (workspace dep, currently `=2.0.0-rc.10`) binds to
# a specific major-minor of the ONNX Runtime native library. The expected
# version is encoded in `ort_sys::ORT_API_VERSION` and surfaced at runtime
# as: "expected GetVersionString to return '1.{N}.x'".
#
# Discovered loudly via the T0.1.7 Phase 1 runtime confirmation
# (HANDOFF.md "Phase 1 runtime confirmation finding"): the script
# initially picked ORT 1.20.0 arbitrarily, ort rc.10 rejected it.
#
# Current pinning:
#   ort        = "=2.0.0-rc.10"  (workspace Cargo.toml)
#   $OrtVersion = "1.22.0"        (this script - must match ort's expected major-minor)
#
# **When bumping `ort`**: check the ort release notes for the new bundled
# ONNX Runtime version and update $OrtVersion here AND in setup-dev-env.sh.
# The recurring monthly ort-RC-policy check (HANDOFF.md tech-debt) naturally
# catches this - when bumping ort, also verify this script's $OrtVersion.
# ----------------------------------------------------------------------

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..")
$FixtureDir = Join-Path $RepoRoot "crates\vault-embedding\test-fixtures\bge-small-en-v1.5"

$ExpectedModelSha256 = "828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35"
$ModelUrl = "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx?download=true"
$TokenizerUrl = "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json"

$OrtVersion = "1.22.0"  # MUST match ort crate version's bundled ORT (see header above).
$OrtAsset = "onnxruntime-win-x64-${OrtVersion}.zip"
$OrtUrl = "https://github.com/microsoft/onnxruntime/releases/download/v${OrtVersion}/${OrtAsset}"
# SHA-256 of $OrtAsset for v1.22.0, recorded 2026-10-06 (security audit s89).
$ExpectedOrtSha256 = "174c616efc0271194488642a72f1a514e01487da4dfe84c49296d66e40ebe0da"

New-Item -ItemType Directory -Path $FixtureDir -Force | Out-Null
Set-Location $FixtureDir

function Sha256OfFile($path) {
    return (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLower()
}

function Download-IfNeeded($file, $url, $expectedSha256) {
    if (Test-Path $file) {
        if ($expectedSha256) {
            $actual = Sha256OfFile $file
            if ($actual -eq $expectedSha256) {
                Write-Host "  OK $file (already present, SHA-256 matches)"
                return
            } else {
                Write-Host "  WARN $file SHA-256 mismatch - re-downloading"
                Write-Host "       expected: $expectedSha256"
                Write-Host "       actual:   $actual"
                Remove-Item $file
            }
        } else {
            Write-Host "  OK $file (already present)"
            return
        }
    }

    Write-Host "  -> downloading $file from $url"
    Invoke-WebRequest -Uri $url -OutFile $file -UseBasicParsing

    if ($expectedSha256) {
        $actual = Sha256OfFile $file
        if ($actual -ne $expectedSha256) {
            Write-Error "SHA-256 mismatch after download for $file`n       expected: $expectedSha256`n       actual:   $actual`n       This may indicate the upstream file changed - verify against MODEL_PROVENANCE.md"
            exit 1
        }
        Write-Host "  OK $file (SHA-256 verified)"
    }
}

Write-Host "vault-embedding dev fixtures -> $FixtureDir"
Write-Host ""

Write-Host "Model + tokenizer:"
Download-IfNeeded "model.onnx" $ModelUrl $ExpectedModelSha256
Download-IfNeeded "tokenizer.json" $TokenizerUrl $null

$TokenizerSha256 = Sha256OfFile "tokenizer.json"
Write-Host "  tokenizer.json SHA-256 = $TokenizerSha256"
Write-Host "  (paste into BGE_SMALL_EN_V1_5_TOKENIZER_SHA256 in src/integrity.rs if still placeholder)"
Write-Host ""

Write-Host "ONNX Runtime native lib (v${OrtVersion}):"
if (Test-Path "onnxruntime.dll") {
    Write-Host "  OK onnxruntime.dll (already present)"
} else {
    Write-Host "  -> downloading $OrtAsset from $OrtUrl"
    Invoke-WebRequest -Uri $OrtUrl -OutFile $OrtAsset -UseBasicParsing
    # The runtime ships inside the installer, so the archive is checked like
    # the model (security audit s89). A new $OrtVersion needs a new hash.
    $OrtActual = Sha256OfFile $OrtAsset
    if ($OrtActual -ne $ExpectedOrtSha256) {
        Remove-Item $OrtAsset
        Write-Error "SHA-256 mismatch for $OrtAsset`n       expected: $ExpectedOrtSha256`n       actual:   $OrtActual"
        exit 1
    }
    Write-Host "  OK $OrtAsset (SHA-256 verified)"
    Expand-Archive -Path $OrtAsset -DestinationPath . -Force
    $InnerDll = Join-Path "onnxruntime-win-x64-${OrtVersion}\lib" "onnxruntime.dll"
    Copy-Item $InnerDll "onnxruntime.dll"
    Remove-Item $OrtAsset
    Remove-Item -Recurse "onnxruntime-win-x64-${OrtVersion}"
    Write-Host "  OK onnxruntime.dll (downloaded + extracted)"
}

Write-Host ""
Write-Host "Done. vault-embedding tests can now run via:"
Write-Host "  cargo test -p vault-embedding"
