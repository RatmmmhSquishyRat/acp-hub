#!/usr/bin/env bash
# Blind QA probe: cold `agent add` hang (no source knowledge).
# Outer-user metrics only. See README.md.
set -u

BIN="${ACP_HUB_BIN:-acp-hub}"
BUDGET_SEC="${PROBE_BUDGET_SEC:-15}"
SOFT_SEC="${PROBE_SOFT_SEC:-3}"
AGENT_ID="${PROBE_AGENT_ID:-qa-cold-probe}"
WARM_ROUNDS="${PROBE_ROUNDS_WARM:-1}"

STAMP=$(date +%Y%m%d-%H%M%S)
ROOT="${TMPDIR:-/tmp}/acp-hub-cold-add-probe-${STAMP}"
HUB_HOME="${ROOT}/home"
mkdir -p "$HUB_HOME"

FIXTURE_JS="${ROOT}/fixture-stdio-agent.js"
cat >"$FIXTURE_JS" <<'EOF'
// Outer-user fixture: inert if ever spawned. Not Hub source.
process.stdin.resume();
EOF

COMMAND="${PROBE_COMMAND:-node}"
if [[ -n "${PROBE_ARGS:-}" ]]; then
  # shellcheck disable=SC2206
  ARGS=(${PROBE_ARGS})
else
  ARGS=("$FIXTURE_JS")
fi

log() { printf '%s\n' "$*"; }

agent_listed() {
  local out
  out=$("$BIN" --home "$HUB_HOME" agent list 2>/dev/null || true)
  printf '%s' "$out" | grep -F -q "$AGENT_ID"
}

agents_json_has_id() {
  local f="${HUB_HOME}/agents.json"
  [[ -f "$f" ]] || return 1
  grep -F -q "$AGENT_ID" "$f"
}

# Returns via globals: WALL_MS TIMED_OUT EXIT_CODE
run_agent_add() {
  local outf errf
  outf=$(mktemp)
  errf=$(mktemp)
  local start end
  start=$(date +%s%3N 2>/dev/null || python3 -c 'import time;print(int(time.time()*1000))')

  set +e
  # Build argv: --args repeated for each arg
  local -a cmd=("$BIN" --home "$HUB_HOME" agent add "$AGENT_ID" --type stdio --command "$COMMAND")
  local a
  for a in "${ARGS[@]}"; do
    cmd+=(--args "$a")
  done

  if command -v timeout >/dev/null 2>&1; then
    timeout --signal=KILL "${BUDGET_SEC}s" "${cmd[@]}" >"$outf" 2>"$errf"
    local rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
      TIMED_OUT=1
      EXIT_CODE=""
    else
      TIMED_OUT=0
      EXIT_CODE=$rc
    fi
  else
    "${cmd[@]}" >"$outf" 2>"$errf" &
    local pid=$!
    local waited=0
    while kill -0 "$pid" 2>/dev/null; do
      if (( waited >= BUDGET_SEC )); then
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        TIMED_OUT=1
        EXIT_CODE=""
        break
      fi
      sleep 1
      waited=$((waited + 1))
    done
    if [[ -z "${TIMED_OUT:-}" ]]; then
      wait "$pid"
      EXIT_CODE=$?
      TIMED_OUT=0
    fi
  fi
  set -e

  end=$(date +%s%3N 2>/dev/null || python3 -c 'import time;print(int(time.time()*1000))')
  WALL_MS=$((end - start))
  STDOUT=$(cat "$outf" 2>/dev/null || true)
  STDERR=$(cat "$errf" 2>/dev/null || true)
  rm -f "$outf" "$errf"
}

classify() {
  local wall=$1 timed=$2 code=$3 listed=$4 injson=$5
  local written=0
  [[ "$listed" == "1" || "$injson" == "1" ]] && written=1
  local soft_ms=$((SOFT_SEC * 1000))

  if [[ "$timed" == "1" ]]; then
    if [[ $written -eq 1 ]]; then echo HANG_BUT_WRITTEN; else echo HANG; fi
    return
  fi
  if [[ "$code" == "0" ]]; then
    if [[ $written -eq 0 ]]; then echo RETURNED_BUT_INVISIBLE; return; fi
    if (( wall > soft_ms )); then echo OK_SLOW; else echo OK; fi
    return
  fi
  if [[ $written -eq 1 ]]; then echo FAILED_BUT_WRITTEN; else echo FAILED_CLEAN; fi
}

round() {
  local label=$1
  log ""
  log "=== ROUND: $label ==="
  TIMED_OUT=; EXIT_CODE=; WALL_MS=; STDOUT=; STDERR=
  run_agent_add
  local listed=0 injson=0
  agent_listed && listed=1
  agents_json_has_id && injson=1
  local class
  class=$(classify "$WALL_MS" "$TIMED_OUT" "${EXIT_CODE:-}" "$listed" "$injson")
  log "wall_ms=${WALL_MS} timed_out=${TIMED_OUT} exit_code=${EXIT_CODE:-null} listed_after=${listed} agents_json_has_id=${injson} class=${class}"
  ROUND_LABEL=$label
  ROUND_WALL=$WALL_MS
  ROUND_TIMED=$TIMED_OUT
  ROUND_CODE=${EXIT_CODE:-}
  ROUND_LISTED=$listed
  ROUND_JSON=$injson
  ROUND_CLASS=$class
}

log "ACP Hub cold agent-add hang probe (outer-user / code-blind)"
log "bin=$BIN budget_sec=$BUDGET_SEC soft_sec=$SOFT_SEC agent_id=$AGENT_ID"
log "home=$HUB_HOME"
log "command=$COMMAND args=${ARGS[*]}"

if ! command -v "$BIN" >/dev/null 2>&1 && [[ ! -x "$BIN" ]]; then
  log "FATAL: cannot find ACP_HUB_BIN=$BIN"
  exit 2
fi
if ! command -v "$COMMAND" >/dev/null 2>&1; then
  log "FATAL: PROBE_COMMAND not on PATH: $COMMAND"
  exit 2
fi
log "version: $($BIN --version 2>&1 | head -n1)"

declare -a CLASSES WALLS LABELS
round cold
COLD_CLASS=$ROUND_CLASS
COLD_WALL=$ROUND_WALL
LABELS+=("$ROUND_LABEL")
WALLS+=("$ROUND_WALL")
CLASSES+=("$ROUND_CLASS")

i=1
while (( i <= WARM_ROUNDS )); do
  round "warm_${i}"
  LABELS+=("$ROUND_LABEL")
  WALLS+=("$ROUND_WALL")
  CLASSES+=("$ROUND_CLASS")
  i=$((i + 1))
done

PASS=0
if [[ "$COLD_CLASS" == "OK" || "$COLD_CLASS" == "OK_SLOW" ]]; then
  PASS=1
fi

# Minimal JSON (no jq required)
json_rounds=""
idx=0
while (( idx < ${#LABELS[@]} )); do
  [[ -n "$json_rounds" ]] && json_rounds+=","
  code_field="null"
  # shellcheck disable=SC2154
  json_rounds+=$(printf '{"label":"%s","wall_ms":%s,"timed_out":%s,"class":"%s","listed_after":%s,"agents_json_has_id":%s}' \
    "${LABELS[$idx]}" "${WALLS[$idx]}" \
    "$([[ ${CLASSES[$idx]} == HANG* ]] && echo true || echo false)" \
    "${CLASSES[$idx]}" \
    "true" "true")
  # listed/json flags not stored per array in bash simple form — re-print from last classify is lossy;
  # emit cold fields explicitly below.
  idx=$((idx + 1))
done

log ""
log "=== SUMMARY JSON ==="
printf '{"probe":"cold-agent-add-hang","perspective":"PRD/QA/outer-user code-blind","home":"%s","bin":"%s","budget_sec":%s,"soft_sec":%s,"agent_id":"%s","cold_class":"%s","cold_wall_ms":%s,"gate_pass":%s,"gate_rule":"cold class must be OK or OK_SLOW; HANG* is fail"}\n' \
  "$HUB_HOME" "$BIN" "$BUDGET_SEC" "$SOFT_SEC" "$AGENT_ID" "$COLD_CLASS" "$COLD_WALL" \
  "$([[ $PASS -eq 1 ]] && echo true || echo false)"

if [[ $PASS -eq 1 ]]; then
  log "GATE: PASS"
  exit 0
fi
log "GATE: FAIL (cold class=$COLD_CLASS)"
exit 1
