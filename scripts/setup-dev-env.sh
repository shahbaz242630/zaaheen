#!/usr/bin/env bash
#
# setup-dev-env.sh — one-time per-checkout dev environment setup for vault-embedding.
#
# Downloads:
#   - bge-small-en-v1.5 model.onnx (~133 MB) from BAAI's official Hugging Face repo
#   - bge-small-en-v1.5 tokenizer.json (~711 KB)
#   - ONNX Runtime native lib for host platform (Linux .so / macOS .dylib)
#
# Verifies SHA-256 of model.onnx against the canonical hash in
# crates/vault-embedding/src/integrity.rs. If the tokenizer.json hash placeholder
# is still all-zeroes in integrity.rs, prints the computed hash so the operator
# can paste it into integrity.rs before Phase 2 (per T0.1.7_PLAN.md).
#
# Idempotent: skips downloads if files exist with the correct hash.
# Re-run safely.
#
# For Windows, use scripts/setup-dev-env.ps1 (PowerShell equivalent).
#
# ----------------------------------------------------------------------
# ORT_VERSION ↔ ort crate version coupling — READ BEFORE BUMPING EITHER
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
#   ORT_VERSION = "1.22.0"        (this script — must match ort's expected major-minor)
#
# **When bumping `ort`**: check the ort release notes for the new bundled
# ONNX Runtime version and update ORT_VERSION here AND in setup-dev-env.ps1.
# The recurring monthly ort-RC-policy check (HANDOFF.md tech-debt) naturally
# catches this — when bumping ort, also verify this script's ORT_VERSION.
# ----------------------------------------------------------------------

set -euo pipefail

# Resolve repo root from script location
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/crates/vault-embedding/test-fixtures/bge-small-en-v1.5"

EXPECTED_MODEL_SHA256="828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35"
MODEL_URL="https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx?download=true"
TOKENIZER_URL="https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json"

ORT_VERSION="1.22.0"  # MUST match ort crate version's bundled ORT (see header). A new version needs new ORT_SHA256 values below.
ORT_RELEASE_BASE="https://github.com/microsoft/onnxruntime/releases/download"

mkdir -p "${FIXTURE_DIR}"
cd "${FIXTURE_DIR}"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "ERROR: neither sha256sum nor shasum found on PATH" >&2
        exit 1
    fi
}

download_if_needed() {
    local file="$1"
    local url="$2"
    local expected_sha256="${3:-}"

    if [[ -f "${file}" ]]; then
        if [[ -n "${expected_sha256}" ]]; then
            local actual
            actual="$(sha256_of "${file}")"
            if [[ "${actual}" == "${expected_sha256}" ]]; then
                echo "  OK ${file} (already present, SHA-256 matches)"
                return 0
            else
                echo "  WARN ${file} SHA-256 mismatch — re-downloading"
                echo "       expected: ${expected_sha256}"
                echo "       actual:   ${actual}"
                rm -f "${file}"
            fi
        else
            echo "  OK ${file} (already present)"
            return 0
        fi
    fi

    echo "  -> downloading ${file} from ${url}"
    curl --fail --location --silent --show-error -o "${file}" "${url}"

    if [[ -n "${expected_sha256}" ]]; then
        local actual
        actual="$(sha256_of "${file}")"
        if [[ "${actual}" != "${expected_sha256}" ]]; then
            echo "ERROR: SHA-256 mismatch after download for ${file}"
            echo "       expected: ${expected_sha256}"
            echo "       actual:   ${actual}"
            echo "       This may indicate the upstream file changed — verify against MODEL_PROVENANCE.md"
            exit 1
        fi
        echo "  OK ${file} (SHA-256 verified)"
    fi
}

echo "vault-embedding dev fixtures -> ${FIXTURE_DIR}"
echo

echo "Model + tokenizer:"
download_if_needed "model.onnx" "${MODEL_URL}" "${EXPECTED_MODEL_SHA256}"
download_if_needed "tokenizer.json" "${TOKENIZER_URL}" ""

# Compute and report tokenizer SHA-256 so operator can paste into integrity.rs
TOKENIZER_SHA256="$(sha256_of "tokenizer.json")"
echo "  tokenizer.json SHA-256 = ${TOKENIZER_SHA256}"
echo "  (paste into BGE_SMALL_EN_V1_5_TOKENIZER_SHA256 in src/integrity.rs if still placeholder)"
echo

echo "ONNX Runtime native lib (v${ORT_VERSION}):"
case "$(uname -s)" in
    Linux*)
        ORT_ASSET="onnxruntime-linux-x64-${ORT_VERSION}.tgz"
        ORT_LIB_NAME="libonnxruntime.so"
        ORT_INNER_PATH="onnxruntime-linux-x64-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION}"
        ORT_SHA256="8344d55f93d5bc5021ce342db50f62079daf39aaafb5d311a451846228be49b3"
        ;;
    Darwin*)
        ORT_ASSET="onnxruntime-osx-arm64-${ORT_VERSION}.tgz"
        ORT_LIB_NAME="libonnxruntime.dylib"
        ORT_INNER_PATH="onnxruntime-osx-arm64-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib"
        ORT_SHA256="cab6dcbd77e7ec775390e7b73a8939d45fec3379b017c7cb74f5b204c1a1cc07"
        ;;
    *)
        echo "ERROR: unsupported platform $(uname -s) — use scripts/setup-dev-env.ps1 on Windows"
        exit 1
        ;;
esac

if [[ -f "${ORT_LIB_NAME}" ]]; then
    echo "  OK ${ORT_LIB_NAME} (already present)"
else
    ORT_URL="${ORT_RELEASE_BASE}/v${ORT_VERSION}/${ORT_ASSET}"
    echo "  -> downloading ${ORT_ASSET} from ${ORT_URL}"
    curl --fail --location --silent --show-error -o "${ORT_ASSET}" "${ORT_URL}"
    # The runtime ships inside the installer, so the archive is checked like
    # the model (security audit s89). Hashes are for v1.22.0, recorded
    # 2026-10-06; a new ORT_VERSION needs new ones.
    ort_actual="$(sha256_of "${ORT_ASSET}")"
    if [[ "${ort_actual}" != "${ORT_SHA256}" ]]; then
        rm -f "${ORT_ASSET}"
        echo "ERROR: SHA-256 mismatch for ${ORT_ASSET}" >&2
        echo "       expected: ${ORT_SHA256}" >&2
        echo "       actual:   ${ort_actual}" >&2
        exit 1
    fi
    echo "  OK ${ORT_ASSET} (SHA-256 verified)"
    tar -xzf "${ORT_ASSET}"
    cp "${ORT_INNER_PATH}" "${ORT_LIB_NAME}"
    rm -rf "${ORT_ASSET}" "$(dirname "$(dirname "${ORT_INNER_PATH}")")"
    echo "  OK ${ORT_LIB_NAME} (downloaded + extracted)"
fi

echo
echo "Done. vault-embedding tests can now run via:"
echo "  cargo test -p vault-embedding"
