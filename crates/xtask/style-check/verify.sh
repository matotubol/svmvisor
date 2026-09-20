#!/usr/bin/env bash
# Style-cleanup gate (CODINGSTYLE.md section 13). Run from the repository root:
#
#   bash crates/xtask/style-check/verify.sh quick
#       format check + every test configuration
#   bash crates/xtask/style-check/verify.sh full
#       quick + UEFI build + resident pipeline + disassembly comparison against
#       the baseline in target/style-baseline/disassembly.log
#   bash crates/xtask/style-check/verify.sh baseline [REV]
#       (re)build that baseline from REV in a temporary git worktree.
#       REV defaults to a77f713, the last commit before the style cleanup.
#
# Exit status is non-zero when any step fails or any function body differs.
set -u
MODE="${1:-full}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="target/style-baseline"
BASE_LOG="$BASE_DIR/disassembly.log"
FAILED=0

if [ "$MODE" = "baseline" ]; then
    REV="${2:-a77f713}"
    WORKTREE="target/style-baseline-worktree"
    git worktree remove --force "$WORKTREE" >/dev/null 2>&1
    git worktree add --detach "$WORKTREE" "$REV" >/dev/null 2>&1 || { echo "cannot check out $REV"; exit 1; }
    (cd "$WORKTREE" && cargo xtask resident --output baseline-output --low-runtime >/dev/null 2>&1)
    RC=$?
    if [ "$RC" -eq 0 ]; then
        mkdir -p "$BASE_DIR"
        cp "$WORKTREE/baseline-output/disassembly.log" "$BASE_LOG"
        git rev-parse "$REV" > "$BASE_DIR/revision"
        echo "baseline written to $BASE_LOG from $(git rev-parse --short "$REV")"
    else
        echo "cargo xtask resident failed at $REV (rc=$RC)"
    fi
    git worktree remove --force "$WORKTREE" >/dev/null 2>&1
    exit "$RC"
fi

step() {
    local label="$1"; shift
    local out rc tally
    out="$("$@" 2>&1)"; rc=$?
    tally="$(printf '%s\n' "$out" | awk '/^test result/{p+=$4; f+=$6} END{if(p+f>0) printf "passed=%d failed=%d", p, f}')"
    printf '%-62s rc=%d %s\n' "$label" "$rc" "$tally"
    if [ "$rc" -ne 0 ]; then
        FAILED=1
        printf '%s\n' "$out" | grep -E '^(error|warning: unused)|panicked|FAILED|failed' | head -15 | sed 's/^/    /'
    fi
}

step "fmt workspace"            cargo fmt --all -- --check
step "fmt firmware-handoff"     cargo fmt --manifest-path crates/firmware-handoff/Cargo.toml -- --check
step "fmt resident-payload"     cargo fmt --manifest-path crates/resident-payload/Cargo.toml -- --check
step "test workspace"           cargo test --workspace
# The resident-runtime features only link with --lib: the integration tests
# cannot resolve the runtime's assembly symbols.
step "test hypervisor resident-runtime (lib)"      cargo test -p svmvisor-hypervisor --lib --features resident-runtime
step "test hypervisor resident-runtime-test (lib)" cargo test -p svmvisor-hypervisor --lib --features resident-runtime-test
for feature in native-returning native-resident-boot native-transition-multi-exit memory-attribute-probe; do
    step "test launcher $feature" cargo test -p svmvisor-launcher --features "$feature"
done
for feature in card-load-only card-returning-loader card-resident-dev-loader card-resident-loader; do
    step "test card-loader $feature" cargo test -p svmvisor-card-loader --features "$feature"
done
step "test firmware-handoff"    cargo test --manifest-path crates/firmware-handoff/Cargo.toml

if [ "$MODE" = "full" ]; then
    step "build-card-loader card-resident-dev-loader" cargo build-card-loader --features card-resident-dev-loader
    OUT="target/native-resident/verify-$$-$(date +%s)"
    step "xtask resident" cargo xtask resident --output "$OUT" --low-runtime
    if [ ! -f "$BASE_LOG" ]; then
        FAILED=1
        echo "no baseline: run 'bash crates/xtask/style-check/verify.sh baseline' first"
    elif [ -f "$OUT/disassembly.log" ]; then
        echo "--- disassembly vs baseline (layout-insensitive)"
        python "$HERE/asmdiff.py" "$BASE_LOG" "$OUT/disassembly.log" --show 25 \
            || { FAILED=1; echo "    (differences listed above: explain each one)"; }
    fi
    rm -rf "$OUT"
fi

# Warnings are part of the contract: style work must not introduce any.
WARNINGS="$(cargo check --workspace --all-targets 2>&1 | grep -c '^warning' || true)"
echo "cargo check warnings (workspace, default features): $WARNINGS"
[ "$FAILED" -eq 0 ] && echo "VERIFY: OK" || echo "VERIFY: FAILED"
exit "$FAILED"
